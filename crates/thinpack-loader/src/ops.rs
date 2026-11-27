// `op2` hands the state handle to the op by value; every op body only borrows it.
#![expect(
    clippy::needless_pass_by_value,
    reason = "op2 requires `Rc<RefCell<OpState>>` as the state argument"
)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use deno_core::error::CoreErrorKind;
use deno_core::v8;
use deno_core::{ModuleSpecifier, OpDecl, OpState, op2};
use deno_error::JsErrorBox;
use thinpack_core::{BuiltinSources, file_url};
use thinpack_resolve::{FileSystem, Format, Resolved};
use url::Url;

#[cfg(feature = "esm-hooks")]
use crate::error::HookError;
use crate::error::LoaderError;
use crate::{EsmLoader, Main, ModuleFs, missing_builtin_message, posix_separators};

pub(crate) const OPS: &[OpDecl] = &[
    op_require(),
    op_require_esm(),
    op_entry(),
    op_module_file_url(),
    op_builtin_modules(),
    op_require_resolve(),
    op_require_resolve_paths(),
    op_module_lookup_paths(),
    op_module_global_paths(),
    op_esm_resolve_default(),
    op_esm_load_default(),
    op_esm_format(),
    op_esm_async_active(),
    #[cfg(feature = "esm-hooks")]
    op_esm_async_resolve(),
    #[cfg(feature = "esm-hooks")]
    op_esm_async_load(),
    #[cfg(feature = "esm-hooks")]
    op_esm_register(),
];

#[op2]
#[string]
fn op_require_resolve(
    state: Rc<RefCell<OpState>>,
    #[string] specifier: &str,
    #[string] parent: &str,
) -> Result<String, JsErrorBox> {
    require_resolve(state, specifier, parent, None)
}

#[op2]
#[string]
fn op_require_resolve_paths(
    state: Rc<RefCell<OpState>>,
    #[string] specifier: &str,
    #[string] parent: &str,
    #[serde] paths: Vec<String>,
) -> Result<String, JsErrorBox> {
    let paths: Vec<PathBuf> = paths.iter().map(|path| search_path(path)).collect();
    require_resolve(state, specifier, parent, Some(&paths))
}

fn require_resolve(
    state: Rc<RefCell<OpState>>,
    specifier: &str,
    parent: &str,
    search: Option<&[PathBuf]>,
) -> Result<String, JsErrorBox> {
    let op_state = state.borrow();
    let builtins = op_state.borrow::<BuiltinSources>();
    if let Some(name) = builtin_specifier(builtins, specifier) {
        return Ok(name);
    }
    if thinpack_caps::module_for_builtin(specifier).is_some() {
        return Err(JsErrorBox::from_err(LoaderError::new(
            "ERR_UNKNOWN_BUILTIN_MODULE",
            missing_builtin_message(specifier),
        )));
    }
    let fs = op_state.borrow::<ModuleFs>();
    let parent_path = Path::new(parent);
    let conditions = fs.conditions(false);
    let resolved = search.map_or_else(
        || thinpack_resolve::resolve_with_conditions(specifier, parent_path, fs, &conditions),
        |paths| {
            thinpack_resolve::resolve_from_paths(specifier, parent_path, paths, fs, &conditions)
        },
    );
    match resolved {
        Ok(resolved) => Ok(fs.js_path(&resolved.path)),
        Err(error) => {
            let message = if matches!(error, thinpack_resolve::Error::NotFound) {
                format!("Cannot find module '{specifier}'\nRequire stack:\n- {parent}")
            } else {
                error.message(|path| fs.js_path(path))
            };
            Err(JsErrorBox::from_err(LoaderError {
                code: resolve_code(&error),
                message,
                url: None,
            }))
        }
    }
}

fn builtin_specifier(builtins: &BuiltinSources, specifier: &str) -> Option<String> {
    builtins
        .get(&builtin_name(specifier))
        .map(|_| specifier.to_owned())
}

// Node exposes these built-ins only through the `node:` scheme; a bare name is
// an ordinary package request.
const NODE_SCHEME_ONLY: &[&str] = &["ffi", "sea", "sqlite", "test"];

fn is_node_builtin(builtins: &BuiltinSources, specifier: &str) -> bool {
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    if bare.starts_with("thinpack/") {
        return false;
    }
    if NODE_SCHEME_ONLY.contains(&bare) && !specifier.starts_with("node:") {
        return false;
    }
    builtins.get(&builtin_name(specifier)).is_some()
        || thinpack_caps::module_for_builtin(bare).is_some()
}

#[op2]
#[string]
fn op_module_lookup_paths(
    state: Rc<RefCell<OpState>>,
    #[string] specifier: &str,
    #[string] parent: &str,
    #[string] global_paths: &str,
) -> String {
    let op_state = state.borrow();
    let builtins = op_state.borrow::<BuiltinSources>();
    if is_node_builtin(builtins, specifier) {
        return "n".to_owned();
    }
    let fs = op_state.borrow::<ModuleFs>();
    let paths = if is_relative_specifier(specifier) {
        vec![fs.js_path(parent_directory(parent))]
    } else {
        let mut paths = if parent.is_empty() {
            Vec::new()
        } else {
            let directory = parent_directory(parent);
            let packed = fs.is_packed(directory);
            thinpack_resolve::node_module_paths(directory)
                .into_iter()
                .map(|path| {
                    let path = path.display().to_string();
                    // The lookup includes /node_modules above the /app mount.
                    if packed { posix_separators(path) } else { path }
                })
                .collect()
        };
        paths.extend(
            global_paths
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_owned),
        );
        paths
    };
    if paths.is_empty() {
        return "n".to_owned();
    }
    let mut reply = String::from("p");
    for path in paths {
        reply.push('\0');
        reply.push_str(&path);
    }
    reply
}

fn is_relative_specifier(specifier: &str) -> bool {
    let bytes = specifier.as_bytes();
    bytes.first() == Some(&b'.')
        && (bytes.len() == 1
            || matches!(bytes[1], b'.' | b'/')
            || (cfg!(windows) && bytes[1] == b'\\'))
}

fn parent_directory(parent: &str) -> &Path {
    Path::new(parent)
        .parent()
        .filter(|directory| !directory.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[op2]
#[string]
fn op_module_global_paths() -> String {
    module_global_paths().join("\0")
}

fn module_global_paths() -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(value) = std::env::var_os("NODE_PATH") {
        let delimiter = if cfg!(windows) { ';' } else { ':' };
        paths.extend(
            value
                .to_string_lossy()
                .split(delimiter)
                .filter(|entry| !entry.is_empty())
                .map(str::to_owned),
        );
    }
    let home_name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    if let Some(home) = std::env::var_os(home_name).filter(|value| !value.is_empty()) {
        let home = absolute(Path::new(&home));
        paths.push(home.join(".node_modules").to_string_lossy().into_owned());
        paths.push(home.join(".node_libraries").to_string_lossy().into_owned());
    }
    if let Ok(executable) = std::env::current_exe() {
        // `$PREFIX/bin/node` on Unix and `$PREFIX\node.exe` on Windows.
        let prefix = if cfg!(windows) {
            executable.parent()
        } else {
            executable.parent().and_then(Path::parent)
        };
        if let Some(prefix) = prefix {
            paths.push(
                prefix
                    .join("lib")
                    .join("node")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    paths
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        thinpack_resolve::normalize(path)
    } else {
        std::env::current_dir().map_or_else(
            |_| thinpack_resolve::normalize(path),
            |directory| thinpack_resolve::normalize(&directory.join(path)),
        )
    }
}

/// The base directory a `require.resolve` search path names. Payload paths stay
/// rooted at `/app` on every platform; relative host paths resolve against the
/// cwd, as they do in Node.
fn search_path(path: &str) -> PathBuf {
    let native = Path::new(path);
    if native.is_absolute() || path.starts_with('/') {
        thinpack_resolve::normalize(native)
    } else {
        absolute(native)
    }
}

#[op2]
#[string]
fn op_esm_resolve_default(
    state: Rc<RefCell<OpState>>,
    #[string] specifier: &str,
    #[string] parent: &str,
) -> Result<String, JsErrorBox> {
    let op_state = state.borrow();
    let fs = op_state.borrow::<ModuleFs>();
    let referrer = if parent.is_empty() {
        let cwd =
            std::env::current_dir().map_err(|error| JsErrorBox::generic(error.to_string()))?;
        Url::from_file_path(cwd.join("[cwd]"))
            .map_err(|()| JsErrorBox::generic("the cwd cannot be represented as a URL"))?
            .to_string()
    } else {
        parent.to_owned()
    };
    crate::esm::resolve(fs, specifier, &referrer).map(|url| url.to_string())
}

#[op2]
#[string]
fn op_esm_load_default(
    state: Rc<RefCell<OpState>>,
    #[string] url: &str,
) -> Result<String, JsErrorBox> {
    let specifier =
        ModuleSpecifier::parse(url).map_err(|error| JsErrorBox::generic(error.to_string()))?;
    let op_state = state.borrow();
    let fs = op_state.borrow::<ModuleFs>();
    let builtins = op_state.borrow::<BuiltinSources>();
    let (format, source) = crate::esm::default_source(fs, builtins, &specifier, None)?;
    Ok(format!("{format}\0{source}"))
}

#[op2]
#[string]
fn op_esm_format(state: Rc<RefCell<OpState>>, #[string] url: &str) -> String {
    let Ok(specifier) = ModuleSpecifier::parse(url) else {
        return String::new();
    };
    let op_state = state.borrow();
    let fs = op_state.borrow::<ModuleFs>();
    let builtins = op_state.borrow::<BuiltinSources>();
    crate::esm::default_format(fs, builtins, &specifier)
        .unwrap_or_default()
        .to_owned()
}

#[op2]
#[string]
fn op_builtin_modules(state: Rc<RefCell<OpState>>) -> String {
    let state = state.borrow();
    let builtins = state.borrow::<BuiltinSources>();
    builtins.names().join("\0")
}

#[op2]
#[string]
fn op_require(
    state: Rc<RefCell<OpState>>,
    #[string] specifier: &str,
    #[string] parent: &str,
) -> String {
    let op_state = state.borrow();
    let builtins = op_state.borrow::<BuiltinSources>();
    let fs = op_state.borrow::<ModuleFs>();

    if let Some(source) = lookup_builtin(builtins, specifier) {
        let name = builtin_name(specifier);
        return format!("b\0{name}\0{name}\0{source}");
    }
    if specifier.starts_with("node:") || thinpack_caps::module_for_builtin(specifier).is_some() {
        return format!(
            "e\0ERR_UNKNOWN_BUILTIN_MODULE\0{}\0",
            missing_builtin_message(specifier)
        );
    }

    match thinpack_resolve::resolve_with_conditions(
        specifier,
        Path::new(parent),
        fs,
        &fs.conditions(false),
    ) {
        Ok(resolved) => read(&resolved, fs),
        Err(error) => resolve_error(&error, fs),
    }
}

#[op2(fast)]
fn op_esm_async_active(state: Rc<RefCell<OpState>>) -> bool {
    state
        .borrow()
        .try_borrow::<Rc<EsmLoader>>()
        .is_some_and(|loader| loader.async_active())
}

#[cfg(feature = "esm-hooks")]
#[op2]
#[string]
fn op_esm_async_resolve(
    state: Rc<RefCell<OpState>>,
    #[string] specifier: &str,
    #[string] parent: &str,
    #[string] attributes: &str,
    #[string] conditions: &str,
) -> String {
    match loader(&state)
        .and_then(|loader| loader.async_resolve(specifier, parent, attributes, conditions))
    {
        Ok(url) => format!("ok\0{url}"),
        Err(error) => error.encode(),
    }
}

#[op2]
#[string]
#[cfg(feature = "esm-hooks")]
fn op_esm_async_load(
    state: Rc<RefCell<OpState>>,
    #[string] url: &str,
    #[string] format: &str,
    #[string] attributes: &str,
    #[string] conditions: &str,
) -> String {
    match loader(&state).and_then(|loader| loader.async_load(url, format, attributes, conditions)) {
        Ok((format, source)) => format!("ok\0{format}\0{source}"),
        Err(error) => error.encode(),
    }
}

#[op2]
#[string]
#[cfg(feature = "esm-hooks")]
fn op_esm_register(
    state: Rc<RefCell<OpState>>,
    #[string] url: &str,
    #[buffer(copy)] data: Vec<u8>,
) -> String {
    match loader(&state).and_then(|loader| loader.register(url, data)) {
        Ok(()) => "ok".to_owned(),
        Err(error) => error.encode(),
    }
}

#[cfg(feature = "esm-hooks")]
fn loader(state: &Rc<RefCell<OpState>>) -> Result<Rc<EsmLoader>, HookError> {
    state
        .borrow()
        .try_borrow::<Rc<EsmLoader>>()
        .cloned()
        .ok_or_else(|| HookError::new("Error", "the ESM loader hook host is not installed"))
}

#[op2(reentrant)]
fn op_require_esm<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: Rc<RefCell<OpState>>,
    #[string] specifier: &str,
) -> Result<v8::Local<'s, v8::Value>, JsErrorBox> {
    let loader = state.borrow().try_borrow::<Rc<EsmLoader>>().cloned();
    if let Some(loader) = &loader {
        loader.enter_require();
    }
    let result = deno_core::require_esm(scope, specifier);
    if let Some(loader) = &loader {
        loader.leave_require();
    }
    result
        .map(|namespace| v8::Local::<v8::Object>::new(scope, namespace).into())
        .map_err(|error| {
            // The engine reports top-level await as a plain generic error with
            // no code; tag it with the standard `code\0detail` prefix so the
            // loader can recognise it without matching the message prose.
            match error.0.as_ref() {
                CoreErrorKind::TLA => {
                    JsErrorBox::new("Error", format!("ERR_REQUIRE_ASYNC_MODULE\0{error}"))
                }
                _ => JsErrorBox::generic(error.to_string()),
            }
        })
}

#[op2]
#[string]
fn op_entry(state: Rc<RefCell<OpState>>) -> String {
    let op_state = state.borrow();
    let fs = op_state.borrow::<ModuleFs>();
    match op_state.borrow::<Main>() {
        Main::File(path) => file_url(path).map_or_else(
            || invalid_url(&fs.js_path(path)),
            |url| format!("f\0{}\0{url}\0", fs.js_path(path)),
        ),
        Main::Eval(source) => {
            let filename = std::env::current_dir()
                .map_or_else(|_| "[eval]".into(), |directory| directory.join("[eval]"));
            let url = file_url(&filename).map_or_else(|| "thinpack:eval".to_owned(), String::from);
            format!("v\0{}\0{url}\0{source}", fs.js_path(&filename))
        }
    }
}

/// The stack origin for source compiled from memory by `Module.prototype._compile`.
#[op2]
#[string]
fn op_module_file_url(#[string] filename: &str) -> String {
    file_url(Path::new(filename)).map_or_else(|| "thinpack:compile".to_owned(), String::from)
}

fn builtin_name(specifier: &str) -> String {
    if specifier.starts_with("node:") {
        specifier.to_owned()
    } else {
        format!("node:{specifier}")
    }
}

fn lookup_builtin<'a>(builtins: &'a BuiltinSources, specifier: &str) -> Option<&'a str> {
    builtins.get(&builtin_name(specifier))
}

fn read(resolved: &Resolved, fs: &ModuleFs) -> String {
    let path = fs.js_path(&resolved.path);

    if let Some(message) = crate::unstripped_typescript(fs, &resolved.path) {
        return format!("e\0ERR_UNKNOWN_FILE_EXTENSION\0{message}\0");
    }

    let Some(url) = file_url(&resolved.path) else {
        return invalid_url(&path);
    };

    if resolved.format == Format::Esm {
        return format!("m\0{path}\0{url}\0");
    }

    match fs.read(&resolved.path) {
        Ok(bytes) => String::from_utf8(bytes).map_or_else(
            |_| format!("e\0ERR_MODULE_READ\0{path}: module is not valid UTF-8\0"),
            |source| {
                // JSON is parsed by the loader, not compiled; it keeps its raw text.
                if resolved.format == Format::Json {
                    return format!("j\0{path}\0{url}\0{source}");
                }
                // CommonJS source stays unwrapped: the loader wraps it once, and a
                // delegated default `.js` handler compiles it through `_compile`.
                format!("f\0{path}\0{url}\0{}", crate::strip_hashbang(&source))
            },
        ),
        Err(error) => format!("e\0ERR_MODULE_READ\0{path}: {error}\0"),
    }
}

fn invalid_url(path: &str) -> String {
    format!("e\0ERR_INVALID_URL\0{path} cannot be converted to a file URL\0")
}

const fn resolve_code(error: &thinpack_resolve::Error) -> &'static str {
    use thinpack_resolve::Error;

    match error {
        Error::NotFound => "MODULE_NOT_FOUND",
        Error::NotExported { .. } => "ERR_PACKAGE_PATH_NOT_EXPORTED",
        Error::Unsupported { .. } => "ERR_UNKNOWN_FILE_EXTENSION",
        Error::InvalidPackage { .. } => "ERR_INVALID_PACKAGE_CONFIG",
        Error::Io { .. } => "ERR_MODULE_READ",
    }
}

fn resolve_error(error: &thinpack_resolve::Error, fs: &ModuleFs) -> String {
    format!(
        "e\0{}\0{}\0",
        resolve_code(error),
        error.message(|path| fs.js_path(path))
    )
}
