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
use percent_encoding::percent_decode_str;
use thinpack_core::{BuiltinSources, file_url};
use thinpack_resolve::{FileSystem, Format, Resolved};
use url::Url;

use crate::error::LoaderError;
use crate::{Main, ModuleFs, missing_builtin_message};

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
    if thinpack_caps::module_for_builtin(specifier).is_some() {
        return Err(JsErrorBox::from_err(LoaderError::new(
            "ERR_UNKNOWN_BUILTIN_MODULE",
            missing_builtin_message(specifier),
        )));
    }
    let fs = op_state.borrow::<ModuleFs>();
    match thinpack_resolve::resolve(specifier, Path::new(parent), fs) {
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

    match thinpack_resolve::resolve(specifier, Path::new(parent), fs) {
        Ok(resolved) => read(&resolved, fs),
        Err(error) => resolve_error(&error, fs),
    }
}

#[op2(reentrant)]
fn op_require_esm<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    #[string] specifier: &str,
) -> Result<v8::Local<'s, v8::Value>, JsErrorBox> {
    deno_core::require_esm(scope, specifier)
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

fn invalid_url(path: &str) -> String {
    format!("e\0ERR_INVALID_URL\0{path} cannot be converted to a file URL\0")
}

fn resolve_code(error: &thinpack_resolve::Error) -> &'static str {
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
