use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use thinpack_core::{Assets, BACKEND, BuiltinSources, IsolateInputs, Runtime};
use thinpack_loader::{Main, ModuleFs};
use thinpack_pack::Pack;
use thinpack_vfs::Vfs;

use crate::{bootstrap, registry};

/// The payload of a packed executable, shared with worker threads. Workers
/// boot on `std::thread`s while the main thread's `Vfs` is an `Rc`, so each
/// worker rebuilds its own `Vfs` and assets from this `Send` copy instead.
static PACK: OnceLock<Arc<Pack>> = OnceLock::new();

pub(crate) fn run_packed(pack: Pack) -> ExitCode {
    let pack = Arc::new(pack);
    let enabled: Vec<&str> = registry::enabled();
    let template = &pack.manifest.template;
    let own_hash = thinpack_caps::capabilities_hash(&enabled);
    if template.version != env!("CARGO_PKG_VERSION")
        || template.backend != BACKEND
        || template.capabilities_hash != own_hash
    {
        eprintln!(
            "this payload was packed for template {}/{}#{} but the running build is {}/{}#{}; \
             re-pack with the matching template",
            template.version,
            template.backend,
            template.capabilities_hash,
            env!("CARGO_PKG_VERSION"),
            BACKEND,
            own_hash,
        );
        return ExitCode::FAILURE;
    }

    let missing: Vec<&str> = pack
        .manifest
        .capabilities
        .iter()
        .map(String::as_str)
        .filter(|capability| !enabled.contains(capability))
        .collect();

    if !missing.is_empty() {
        eprintln!(
            "this build is missing required capabilities: {}\n\
             rebuild the template with: --with={}",
            missing.join(", "),
            missing.join(", ")
        );
        return ExitCode::FAILURE;
    }

    let input = Main::File(PathBuf::from(&pack.manifest.entry));
    let (vfs, assets) = match packed_parts(&pack) {
        Ok(parts) => parts,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    PACK.set(Arc::clone(&pack))
        .expect("the packed payload is installed once");
    let virtual_files: thinpack_core::VirtualFilesRef = vfs.clone();
    match run(
        input,
        ModuleFs::Packed(vfs),
        assets,
        Some(Packed {
            invocation: std::env::args_os()
                .next()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            files: virtual_files,
        }),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn warm(pack: &Pack) -> Result<usize, Box<dyn std::error::Error>> {
    crate::cache::prepare_bytecode_cache();
    let input = Main::File(PathBuf::from(&pack.manifest.entry));
    let fs = ModuleFs::Packed(Rc::new(Vfs::from_pack(pack)?));
    let mut scripts = Vec::new();
    let mut modules = Vec::new();
    for file in &pack.manifest.files {
        let path = PathBuf::from(&file.path);
        if !thinpack_resolve::is_javascript(&path) {
            continue;
        }
        let Some(contents) = pack.slice_of(file) else {
            continue;
        };
        let Ok(source) = std::str::from_utf8(contents) else {
            continue;
        };
        if thinpack_loader::is_esm(&path, &fs) {
            modules.push(path);
        } else if let Some(script) = thinpack_loader::commonjs_script(&path, source) {
            scripts.push(script);
        }
    }

    let compiled = scripts.len() + modules.len();
    let (extensions, builtins, loader) = extensions(input, fs, &IsolateInputs::new(None))?;
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()?
        .block_on(async move {
            let mut runtime = Runtime::new(extensions, builtins, loader.clone())?;
            runtime.put_state(loader);
            for (name, source) in scripts {
                runtime.execute_script(&name, source)?;
            }
            for path in modules {
                runtime.precompile_esm(&path).await?;
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        })?;
    Ok(compiled)
}

pub(crate) struct Packed {
    pub invocation: String,
    pub files: thinpack_core::VirtualFilesRef,
}

pub(crate) fn run(
    input: Main,
    fs: ModuleFs,
    assets: Assets,
    packed: Option<Packed>,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::cache::prepare_bytecode_cache();
    #[cfg(feature = "node-worker-threads")]
    thinpack_core::worker::install_boot(run_worker);

    let main = input;
    let entry = match &main {
        Main::File(path) => Some((path.clone(), thinpack_loader::is_esm(path, &fs))),
        Main::Eval(_) => None,
    };
    let isolate = IsolateInputs::new(packed.as_ref().map(|packed| packed.files.clone()));
    let (extensions, builtins, loader) = extensions(main, fs, &isolate)?;

    let outcome = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()?
        .block_on(async move {
            let mut runtime = Runtime::new(extensions, builtins, loader.clone())?;
            runtime.put_state(loader);
            if let Some(packed) = packed {
                runtime.install_invocation(&thinpack_core::Invocation(packed.invocation))?;
            }
            runtime.install_assets(assets);
            runtime.execute_script(
                "<child-ipc>",
                "if (typeof globalThis.__thinpack_child_ipc_bootstrap__ === \"function\") \
                 globalThis.__thinpack_child_ipc_bootstrap__();"
                    .to_owned(),
            )?;
            runtime.execute_script(
                "<cluster>",
                "if (typeof globalThis.__thinpack_cluster_bootstrap__ === \"function\") \
                 globalThis.__thinpack_cluster_bootstrap__();"
                    .to_owned(),
            )?;
            execute(&mut runtime, entry).await
        });
    Ok(outcome?)
}

async fn execute(
    runtime: &mut Runtime,
    entry: Option<(PathBuf, bool)>,
) -> Result<(), thinpack_core::Error> {
    if let Some((path, true)) = entry {
        runtime.run_esm_file(&path).await?;
    } else {
        thinpack_loader::run(runtime)?;
        runtime.run_event_loop().await?;
    }
    runtime.execute_script("<finish>", "__thinpack_finish()".to_owned())?;
    Ok(())
}

type RuntimeParts = (
    Vec<deno_core::Extension>,
    BuiltinSources,
    Rc<thinpack_loader::EsmLoader>,
);

fn base_extensions(main: Main, fs: ModuleFs, isolate: &IsolateInputs) -> Vec<deno_core::Extension> {
    let mut extensions = vec![
        thinpack_loader::init(main, fs),
        thinpack_io::init(),
        thinpack_node_internals::init(),
        bootstrap::init(),
    ];
    extensions.extend(registry::extensions(isolate));
    extensions
}

fn extensions(
    main: Main,
    fs: ModuleFs,
    isolate: &IsolateInputs,
) -> Result<RuntimeParts, Box<dyn std::error::Error>> {
    let mut extensions = base_extensions(main, fs.clone(), isolate);
    let builtins = BuiltinSources::collect(&mut extensions)?;
    #[cfg(feature = "node-module")]
    let loader = {
        let packed = matches!(fs, ModuleFs::Packed(_));
        thinpack_loader::esm_loader_with_hooks(
            fs,
            builtins.clone(),
            Box::new(move || hook_parts(packed)),
        )
    };
    #[cfg(not(feature = "node-module"))]
    let loader = thinpack_loader::esm_loader(fs, builtins.clone());
    Ok((extensions, builtins, loader))
}

/// Builds the isolate that runs `module.register` hooks. It is created on the hook
/// thread from the same payload and capability set as the app's isolate.
#[cfg(feature = "node-module")]
fn hook_parts(packed: bool) -> Result<thinpack_loader::HookRuntimeParts, String> {
    let (fs, assets, isolate) = if packed {
        let pack = PACK
            .get()
            .ok_or_else(|| "the packed payload is not installed".to_owned())?;
        let (vfs, assets) = packed_parts(pack).map_err(|error| error.to_string())?;
        let isolate = IsolateInputs::new(Some(vfs.clone()));
        (ModuleFs::Packed(vfs), assets, isolate)
    } else {
        (ModuleFs::Host, Assets::host(), IsolateInputs::new(None))
    };
    let mut extensions = base_extensions(Main::Eval(String::new()), fs.clone(), &isolate);
    let builtins = BuiltinSources::collect(&mut extensions).map_err(|error| error.to_string())?;
    let loader: Rc<dyn deno_core::ModuleLoader> = thinpack_loader::esm_loader(fs, builtins.clone());
    Ok(thinpack_loader::HookRuntimeParts {
        extensions,
        builtins,
        assets,
        loader,
    })
}

#[cfg(feature = "node-worker-threads")]
fn run_worker(spec: thinpack_core::worker::WorkerSpec) -> Result<(), thinpack_core::Error> {
    let entry = spec.script.clone();
    let (fs, assets, isolate) = match PACK.get().map(|pack| packed_parts(pack)) {
        Some(Ok((vfs, assets))) => {
            let isolate = IsolateInputs::new(Some(vfs.clone()));
            (ModuleFs::Packed(vfs), assets, isolate)
        }
        Some(Err(error)) => return Err(thinpack_core::Error::new(error.to_string())),
        None => (ModuleFs::Host, Assets::host(), IsolateInputs::new(None)),
    };
    let esm = thinpack_loader::is_esm(&entry, &fs);
    let (extensions, builtins, loader) = extensions(Main::File(entry.clone()), fs, &isolate)
        .map_err(|error| thinpack_core::Error::new(error.to_string()))?;

    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .map_err(thinpack_core::Error::from)?
        .block_on(async move {
            let mut runtime = Runtime::new(extensions, builtins, loader.clone())?;
            runtime.put_state(loader);
            runtime.install_worker_context(spec.context());
            runtime.install_assets(assets);
            execute(&mut runtime, Some((entry, esm))).await
        })
}

/// Rebuilds the per-runtime virtual fs and assets from the payload. Asset
/// slices outside the blob are corruption, so they fail like the file records
/// in `Vfs::from_pack` instead of being skipped.
fn packed_parts(pack: &Pack) -> Result<(Rc<Vfs>, Assets), thinpack_pack::Error> {
    let mut assets = Assets::packed();
    for key in pack.asset_keys() {
        let Some(contents) = pack.asset(key) else {
            return Err(thinpack_pack::Error::Invalid(format!(
                "{key}: missing from the payload blob"
            )));
        };
        assets.insert(key.to_owned(), contents.to_vec());
    }
    Ok((Rc::new(Vfs::from_pack(pack)?), assets))
}
