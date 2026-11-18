use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use thinpack_core::{Assets, BACKEND, BuiltinSources, Runtime};
use thinpack_loader::{Main, ModuleFs};
use thinpack_pack::Pack;
use thinpack_vfs::Vfs;

use crate::{bootstrap, registry};

pub(crate) fn run_packed(pack: &Pack) -> ExitCode {
    let enabled: Vec<&str> = registry::enabled();
    let template = &pack.manifest.template;
    let own_hash = thinpack_caps::capabilities_hash(&enabled);
    if template.version != env!("CARGO_PKG_VERSION")
        || template.backend != BACKEND.to_string()
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
    let mut assets = Assets::packed();
    for key in pack.asset_keys() {
        if let Some(contents) = pack.asset(key) {
            assets.insert(key.to_owned(), contents.to_vec());
        }
    }
    let vfs = match Vfs::from_pack(pack) {
        Ok(vfs) => Rc::new(vfs),
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let virtual_files: thinpack_core::VirtualFilesRef = vfs.clone();
    match run(
        input,
        ModuleFs::Virtual(vfs),
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
    let fs = ModuleFs::Virtual(Rc::new(Vfs::from_pack(pack)?));
    let mut scripts = Vec::new();
    let mut modules = Vec::new();
    for file in &pack.manifest.files {
        if !matches!(
            Path::new(&file.path)
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("js" | "cjs" | "mjs")
        ) {
            continue;
        }
        let Some(contents) = pack.slice_of(file) else {
            continue;
        };
        let Ok(source) = std::str::from_utf8(contents) else {
            continue;
        };
        let path = PathBuf::from(&file.path);
        if thinpack_loader::is_esm(&path, &fs) {
            modules.push(path);
        } else {
            scripts.push((
                format!("file://{}", file.path),
                format!(
                    "(function (exports, require, module, __filename, __dirname) {{{source}\n}})"
                ),
            ));
        }
    }

    let compiled = scripts.len() + modules.len();
    let (extensions, builtins, loader) = extensions(input, fs)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()?
        .block_on(async move {
            let mut runtime = Runtime::new(extensions, builtins, loader)?;
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
    thinpack_core::worker::install_boot(run_worker);

    let main = input;
    let entry = match &main {
        Main::File(path) => Some((path.clone(), thinpack_loader::is_esm(path, &fs))),
        Main::Eval(_) => None,
    };
    let (extensions, builtins, loader) = extensions(main, fs)?;

    let outcome = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()?
        .block_on(async move {
            let mut runtime = Runtime::new(extensions, builtins, loader)?;
            if let Some(packed) = packed {
                runtime.install_invocation(&thinpack_core::Invocation(packed.invocation))?;
                #[cfg(feature = "node-fs")]
                runtime.put_state(thinpack_node_fs::VirtualMount::new(packed.files));
                #[cfg(not(feature = "node-fs"))]
                let _ = packed.files;
            }
            runtime.install_assets(assets);
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
    Rc<dyn deno_core::ModuleLoader>,
);

fn extensions(main: Main, fs: ModuleFs) -> Result<RuntimeParts, Box<dyn std::error::Error>> {
    let mut extensions = vec![
        thinpack_loader::init(main, fs.clone()),
        thinpack_io::init(),
        thinpack_node_internals::init(),
        bootstrap::init(),
    ];
    extensions.extend(registry::extensions());
    #[cfg(feature = "node-ffi")]
    extensions.push(thinpack_node_ffi::engine());
    let builtins = BuiltinSources::collect(&extensions)?;
    let loader = thinpack_loader::esm_loader(fs, builtins.clone());
    Ok((extensions, builtins, loader))
}

fn run_worker(spec: thinpack_core::worker::WorkerSpec) -> Result<(), thinpack_core::Error> {
    let fs = ModuleFs::Host;
    let entry = spec.script.clone();
    let esm = thinpack_loader::is_esm(&entry, &fs);
    let (extensions, builtins, loader) = extensions(Main::File(entry.clone()), fs)
        .map_err(|error| thinpack_core::Error::new(error.to_string()))?;

    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .map_err(thinpack_core::Error::from)?
        .block_on(async move {
            let mut runtime = Runtime::new(extensions, builtins, loader)?;
            runtime.install_worker_context(spec.context());
            runtime.install_assets(Assets::host());
            execute(&mut runtime, Some((entry, esm))).await
        })
}
