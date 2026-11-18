// `op2` hands the state handle to the op by value; every op body only borrows it.
#![expect(
    clippy::needless_pass_by_value,
    reason = "op2 requires `Rc<RefCell<OpState>>` as the state argument"
)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use deno_core::v8;
use deno_core::{ModuleSpecifier, OpDecl, OpState, op2};
use deno_error::JsErrorBox;
use percent_encoding::percent_decode_str;
use thinpack_core::BuiltinSources;
use thinpack_resolve::{FileSystem, Format, Resolved};
use url::Url;

use crate::{Main, ModuleFs};

pub(crate) const OPS: &[OpDecl] = &[
    op_require(),
    op_require_esm(),
    op_entry(),
    op_builtin_modules(),
    op_require_resolve(),
    op_esm_resolve_default(),
    op_esm_load_default(),
];

#[op2]
#[string]
fn op_require_resolve(
    state: Rc<RefCell<OpState>>,
    #[string] specifier: &str,
    #[string] parent: &str,
) -> Result<String, JsErrorBox> {
    let op_state = state.borrow();
    let builtins = op_state.borrow::<BuiltinSources>();
    if let Some(name) = builtin_specifier(builtins, specifier) {
        return Ok(name);
    }
    let fs = op_state.borrow::<ModuleFs>();
    match thinpack_resolve::resolve(specifier, Path::new(parent), fs) {
        Ok(resolved) => Ok(resolved.path.to_string_lossy().into_owned()),
        Err(error) => Err(JsErrorBox::new(
            "Error",
            format!("MODULE_NOT_FOUND\0{error}"),
        )),
    }
}

fn builtin_specifier(builtins: &BuiltinSources, specifier: &str) -> Option<String> {
    builtins
        .get(&builtin_name(specifier))
        .map(|_| specifier.to_owned())
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
    crate::esm::resolve(fs, specifier, &referrer)
        .map(|url| url.to_string())
        .map_err(JsErrorBox::generic)
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
    let (format, source) = crate::esm::default_source(fs, builtins, &specifier)?;
    Ok(format!("{format}\0{source}"))
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
        return format!("e\0ERR_UNKNOWN_BUILTIN_MODULE\0No such built-in module: {specifier}\0");
    }

    match thinpack_resolve::resolve(specifier, Path::new(parent), fs) {
        Ok(resolved) => read(&resolved, fs),
        Err(error) => resolve_error(&error),
    }
}

#[op2(reentrant)]
fn op_require_esm<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    #[string] specifier: &str,
) -> Result<v8::Local<'s, v8::Value>, JsErrorBox> {
    deno_core::require_esm(scope, specifier)
        .map(|namespace| v8::Local::<v8::Object>::new(scope, namespace).into())
        .map_err(|error| JsErrorBox::generic(error.to_string()))
}

#[op2]
#[string]
fn op_entry(state: Rc<RefCell<OpState>>) -> String {
    let op_state = state.borrow();
    match op_state.borrow::<Main>() {
        Main::File(path) => file_url(path).map_or_else(
            || invalid_url(path),
            |url| format!("f\0{}\0{url}\0", path.display()),
        ),
        Main::Eval(source) => {
            let filename = std::env::current_dir()
                .map_or_else(|_| "[eval]".into(), |directory| directory.join("[eval]"));
            let url = file_url(&filename).unwrap_or_else(|| "thinpack:eval".to_owned());
            format!("v\0{}\0{url}\0{source}", filename.display())
        }
    }
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
    let path = resolved.path.display().to_string();

    let Some(url) = file_url(&resolved.path) else {
        return invalid_url(&resolved.path);
    };

    if resolved.format == Format::Esm {
        return format!("m\0{path}\0{url}\0");
    }

    match fs.read(&resolved.path) {
        Ok(bytes) => String::from_utf8(bytes).map_or_else(
            |_| format!("e\0ERR_MODULE_READ\0{path}: module is not valid UTF-8\0"),
            |source| {
                let kind = match resolved.format {
                    Format::Json => "j",
                    _ => "f",
                };
                format!("{kind}\0{path}\0{url}\0{}", strip_hashbang(&source))
            },
        ),
        Err(error) => format!("e\0ERR_MODULE_READ\0{path}: {error}\0"),
    }
}

fn strip_hashbang(source: &str) -> std::borrow::Cow<'_, str> {
    source.strip_prefix("#!").map_or_else(
        || std::borrow::Cow::Borrowed(source),
        |rest| std::borrow::Cow::Owned(format!("//{rest}")),
    )
}

/// Virtual payload paths keep the platform-independent `/app` root. On
/// Windows that is not a host-absolute path, so `Url::from_file_path` rejects
/// it; add a synthetic drive, then remove it from the resulting URL so the
/// specifier stays identical to the Unix one.
#[cfg(windows)]
fn virtual_file_url(path: &Path) -> Option<String> {
    let mut prefixed = std::ffi::OsString::from("C:");
    prefixed.push(path.as_os_str());
    let url = Url::from_file_path(Path::new(&prefixed)).ok()?;
    Some(url.as_str().replace("file:///C:", "file://"))
}

#[cfg(not(windows))]
const fn virtual_file_url(_path: &Path) -> Option<String> {
    None
}

pub(crate) fn file_url(path: &Path) -> Option<String> {
    Url::from_file_path(path)
        .ok()
        .map(String::from)
        .or_else(|| virtual_file_url(path))
}

/// Inverse of [`file_url`] for the virtual `/app` paths that
/// `Url::to_file_path` rejects on Windows.
pub(crate) fn path_from_url(url: &Url) -> Option<PathBuf> {
    if let Ok(path) = url.to_file_path() {
        return Some(path);
    }
    (url.scheme() == "file").then(|| {
        let decoded = percent_decode_str(url.path()).decode_utf8_lossy();
        PathBuf::from(decoded.as_ref())
    })
}

fn invalid_url(path: &Path) -> String {
    format!(
        "e\0ERR_INVALID_URL\0{} cannot be converted to a file URL\0",
        path.display()
    )
}

fn resolve_error(error: &thinpack_resolve::Error) -> String {
    use thinpack_resolve::Error;

    let code = match error {
        Error::NotFound => "MODULE_NOT_FOUND",
        Error::NotExported { .. } => "ERR_PACKAGE_PATH_NOT_EXPORTED",
        Error::Unsupported { .. } => "ERR_UNKNOWN_FILE_EXTENSION",
        Error::InvalidPackage { .. } => "ERR_INVALID_PACKAGE_CONFIG",
        Error::Io { .. } => "ERR_MODULE_READ",
    };
    format!("e\0{code}\0{error}\0")
}
