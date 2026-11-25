use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};

use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};

use deno_error::JsErrorBox;
use thinpack_core::js_string_literal;
use thinpack_core::{BuiltinSources, file_url, path_from_url};
use thinpack_resolve::{FileSystem, Format};

use crate::ModuleFs;
use crate::error::{HookError, LoaderError};
#[cfg(feature = "esm-hooks")]
use crate::hook::{HookHost, HookRuntimeFactory};

#[derive(Debug)]
struct HookLoad {
    format: String,
    source: String,
}

pub struct EsmLoader {
    fs: ModuleFs,
    builtins: BuiltinSources,
    /// Export names read from the live `module.exports` of a builtin when its import was
    /// resolved. A static scan cannot see the alias assignments builtins use; Node reads
    /// `ObjectKeys(module.exports)` and builds the facade once per module.
    builtin_exports: RefCell<HashMap<String, BTreeSet<String>>>,
    hook_loads: RefCell<HashMap<String, HookLoad>>,
    #[cfg(feature = "esm-hooks")]
    hook_attributes: RefCell<HashMap<String, String>>,
    #[cfg(feature = "esm-hooks")]
    hook_factory: RefCell<Option<HookRuntimeFactory>>,
    #[cfg(feature = "esm-hooks")]
    hook_host: RefCell<Option<HookHost>>,
    in_hook: Cell<bool>,
    in_require: Cell<bool>,
}

impl fmt::Debug for EsmLoader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("EsmLoader").finish_non_exhaustive()
    }
}

impl EsmLoader {
    #[must_use]
    pub fn new(fs: ModuleFs, builtins: BuiltinSources) -> Self {
        Self {
            fs,
            builtins,
            builtin_exports: RefCell::new(HashMap::new()),
            hook_loads: RefCell::new(HashMap::new()),
            #[cfg(feature = "esm-hooks")]
            hook_attributes: RefCell::new(HashMap::new()),
            #[cfg(feature = "esm-hooks")]
            hook_factory: RefCell::new(None),
            #[cfg(feature = "esm-hooks")]
            hook_host: RefCell::new(None),
            in_hook: Cell::new(false),
            in_require: Cell::new(false),
        }
    }

    #[cfg(feature = "esm-hooks")]
    #[must_use]
    pub fn with_hooks(fs: ModuleFs, builtins: BuiltinSources, hooks: HookRuntimeFactory) -> Self {
        let loader = Self::new(fs, builtins);
        *loader.hook_factory.borrow_mut() = Some(hooks);
        loader
    }

    /// Whether `module.register` hooks are installed and applicable to this load.
    /// `require(esm)` is synchronous in Node and bypasses the asynchronous hooks.
    #[cfg(feature = "esm-hooks")]
    pub(crate) fn async_active(&self) -> bool {
        !self.in_require.get()
            && self
                .hook_host
                .borrow()
                .as_ref()
                .is_some_and(HookHost::active)
    }

    #[cfg(not(feature = "esm-hooks"))]
    pub(crate) const fn async_active(&self) -> bool {
        false
    }

    pub(crate) fn enter_require(&self) {
        self.in_require.set(true);
    }

    pub(crate) fn leave_require(&self) {
        self.in_require.set(false);
    }

    #[cfg(feature = "esm-hooks")]
    pub(crate) fn register(&self, url: &str, data: Vec<u8>) -> Result<(), HookError> {
        let mut host = self.hook_host.borrow_mut();
        if host.is_none() {
            let factory = self.hook_factory.borrow_mut().take().ok_or_else(|| {
                HookError::new("Error", "module.register is unavailable in this loader")
            })?;
            *host = Some(HookHost::start(factory)?);
        }
        let Some(host) = host.as_mut() else {
            return Err(HookError::new(
                "Error",
                "the ESM loader hook host was not started",
            ));
        };
        host.register(url, data)
    }

    #[cfg(feature = "esm-hooks")]
    pub(crate) fn async_resolve(
        &self,
        specifier: &str,
        parent: &str,
        attributes: &str,
        conditions: &str,
    ) -> Result<String, HookError> {
        self.hook_host
            .borrow()
            .as_ref()
            .ok_or_else(|| HookError::new("Error", "the ESM loader hook host is not running"))?
            .resolve(specifier, parent, attributes, conditions)
    }

    #[cfg(feature = "esm-hooks")]
    pub(crate) fn async_load(
        &self,
        url: &str,
        format: &str,
        attributes: &str,
        conditions: &str,
    ) -> Result<(String, String), HookError> {
        self.hook_host
            .borrow()
            .as_ref()
            .ok_or_else(|| HookError::new("Error", "the ESM loader hook host is not running"))?
            .load(url, format, attributes, conditions)
    }

    #[cfg(feature = "esm-hooks")]
    fn hook_conditions(&self) -> String {
        let mut conditions = vec!["node".to_owned(), "import".to_owned()];
        conditions.extend(self.fs.custom_conditions().iter().cloned());
        conditions.join("\0")
    }
}

impl ModuleLoader for EsmLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, JsErrorBox> {
        resolve(&self.fs, specifier, referrer)
    }

    fn resolve_with_scope(
        &self,
        scope: &mut deno_core::v8::PinScope<'_, '_>,
        specifier: &str,
        referrer: &str,
        kind: ResolutionKind,
        import_attributes: &HashMap<String, String>,
    ) -> Result<ModuleSpecifier, JsErrorBox> {
        let attributes = encode_attributes(import_attributes);
        let hook_reply = if self.in_hook.get() {
            Err(JsErrorBox::generic(
                "ERR_ESM_HOOK_REENTRANCY: an ESM loader hook tried to load a module",
            ))
        } else {
            self.in_hook.set(true);
            let reply = call_esm_bridge(scope, specifier, referrer, &attributes);
            self.in_hook.set(false);
            reply
        }?;

        let specifier = if hook_reply.is_empty() {
            self.resolve(specifier, referrer, kind)?
        } else {
            let mut parts = hook_reply.splitn(3, '\0');
            let url = parts.next().unwrap_or_default();
            let format = parts.next();
            let source = parts.next();
            let specifier = ModuleSpecifier::parse(url)
                .map_err(|error| JsErrorBox::generic(error.to_string()))?;
            if let (Some(format), Some(source)) = (format, source) {
                self.hook_loads.borrow_mut().insert(
                    specifier.as_str().to_owned(),
                    HookLoad {
                        format: format.to_owned(),
                        source: source.to_owned(),
                    },
                );
            }
            specifier
        };

        // A template that omits a builtin must fail the import with the loader's own
        // `ERR_UNKNOWN_BUILTIN_MODULE`, so only a builtin this runtime carries is asked for
        // its exports.
        #[cfg(feature = "esm-hooks")]
        if self.hook_host.borrow().is_some() {
            self.hook_attributes
                .borrow_mut()
                .insert(specifier.as_str().to_owned(), attributes.clone());
        }
        let builtin = specifier.as_str();
        if specifier.scheme() == "node"
            && self.builtins.get(builtin).is_some()
            && !self.builtin_exports.borrow().contains_key(builtin)
        {
            let names = call_builtin_exports(scope, builtin)?;
            self.builtin_exports
                .borrow_mut()
                .insert(builtin.to_owned(), names);
        }
        Ok(specifier)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let names = self
            .builtin_exports
            .borrow()
            .get(module_specifier.as_str())
            .cloned();
        if let Some(hook_load) = self.hook_loads.borrow().get(module_specifier.as_str()) {
            let hook_load = HookLoad {
                format: hook_load.format.clone(),
                source: hook_load.source.clone(),
            };
            return ModuleLoadResponse::Sync(load_from_hook(
                &self.fs,
                &self.builtins,
                module_specifier,
                hook_load,
                names.as_ref(),
            ));
        }
        #[cfg(feature = "esm-hooks")]
        if self.async_active() {
            let attributes = self
                .hook_attributes
                .borrow_mut()
                .remove(module_specifier.as_str())
                .unwrap_or_default();
            let conditions = self.hook_conditions();
            let format = default_format(&self.fs, &self.builtins, module_specifier).unwrap_or("");
            let result = self
                .async_load(module_specifier.as_str(), format, &attributes, &conditions)
                .map_err(JsErrorBox::from_err)
                .and_then(|(format, source)| {
                    module_from_format(
                        &self.fs,
                        &self.builtins,
                        module_specifier,
                        &format,
                        source,
                        names.as_ref(),
                    )
                });
            return ModuleLoadResponse::Sync(result);
        }
        let referrer = maybe_referrer.map(|referrer| &referrer.specifier);
        let result = default_source(&self.fs, &self.builtins, module_specifier, referrer).and_then(
            |(format, source)| {
                module_from_format(
                    &self.fs,
                    &self.builtins,
                    module_specifier,
                    format,
                    source,
                    names.as_ref(),
                )
            },
        );
        ModuleLoadResponse::Sync(result)
    }
}

fn encode_attributes(attributes: &HashMap<String, String>) -> String {
    let mut encoded = String::new();
    for (key, value) in attributes {
        if !encoded.is_empty() {
            encoded.push('\0');
        }
        encoded.push_str(key);
        encoded.push('\0');
        encoded.push_str(value);
    }
    encoded
}

fn call_esm_bridge(
    scope: &mut deno_core::v8::PinScope<'_, '_>,
    specifier: &str,
    referrer: &str,
    attributes: &str,
) -> Result<String, JsErrorBox> {
    call_global(
        scope,
        "__thinpack_esm_resolve__",
        "the ESM loader hook bridge is not installed",
        "the ESM loader hook failed",
        &[specifier, referrer, attributes],
    )
}

fn call_builtin_exports(
    scope: &mut deno_core::v8::PinScope<'_, '_>,
    specifier: &str,
) -> Result<BTreeSet<String>, JsErrorBox> {
    let names = call_global(
        scope,
        "__thinpack_builtin_exports__",
        "the builtin export bridge is not installed",
        "the builtin export bridge failed",
        &[specifier],
    )?;
    Ok(names
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect())
}

fn call_global(
    scope: &mut deno_core::v8::PinScope<'_, '_>,
    name: &str,
    missing: &'static str,
    failed: &'static str,
    arguments: &[&str],
) -> Result<String, JsErrorBox> {
    deno_core::v8::tc_scope!(let scope, scope);
    let global = scope.get_current_context().global(scope);
    let name = deno_core::v8::String::new(scope, name).unwrap();
    let bridge = global
        .get(scope, name.into())
        .unwrap_or_else(|| deno_core::v8::undefined(scope).into());
    let Ok(bridge) = deno_core::v8::Local::<deno_core::v8::Function>::try_from(bridge) else {
        return Err(JsErrorBox::generic(missing));
    };
    let mut argv: [deno_core::v8::Local<'_, deno_core::v8::Value>; 3] =
        [deno_core::v8::undefined(scope).into(); 3];
    for (slot, argument) in argv.iter_mut().zip(arguments) {
        *slot = deno_core::v8::String::new(scope, argument).unwrap().into();
    }
    let receiver = deno_core::v8::undefined(scope);
    let result = bridge.call(scope, receiver.into(), &argv[..arguments.len()]);
    if scope.has_caught() {
        let exception = scope.exception().unwrap();
        return Err(JsErrorBox::from_err(read_hook_error(scope, exception)));
    }
    let Some(result) = result else {
        return Err(JsErrorBox::generic(failed));
    };
    Ok(result.to_rust_string_lossy(scope))
}

/// Rebuilds a JavaScript error raised by an ESM loader hook on this side of the
/// bridge. The bridge returns a string, so without this the engine would report a
/// flattened message under a class and code of its own; Node hands a hook's failure
/// to the app as it was thrown, so keep the class, message, `code` and stack.
fn read_hook_error(
    scope: &mut deno_core::v8::PinScope<'_, '_>,
    exception: deno_core::v8::Local<'_, deno_core::v8::Value>,
) -> HookError {
    use deno_core::v8;
    let message = exception.to_rust_string_lossy(scope);
    let Some(object) = exception.to_object(scope) else {
        return HookError::new("Error", message);
    };
    let read = |scope: &mut v8::PinScope<'_, '_>, name: &str| {
        let key = v8::String::new(scope, name).unwrap();
        object
            .get(scope, key.into())
            .filter(|value| value.is_string())
            .map(|value| value.to_rust_string_lossy(scope))
    };
    HookError {
        class: read(scope, "name").unwrap_or_else(|| "Error".to_owned()),
        message: read(scope, "message").unwrap_or(message),
        code: read(scope, "code"),
        stack: read(scope, "stack"),
    }
}

pub(crate) fn resolve(
    fs: &ModuleFs,
    specifier: &str,
    referrer: &str,
) -> Result<ModuleSpecifier, JsErrorBox> {
    let parse_url = |url: &str| {
        ModuleSpecifier::parse(url).map_err(|error| JsErrorBox::generic(error.to_string()))
    };
    if specifier.starts_with("node:") || specifier.starts_with("file:") {
        return parse_url(specifier);
    }
    if thinpack_caps::module_for_builtin(specifier).is_some() {
        return parse_url(&format!("node:{specifier}"));
    }

    let parent_url = (!referrer.is_empty())
        .then(|| parse_url(referrer))
        .transpose()?;
    let parent = parent_url
        .as_ref()
        .map(|url| {
            path_from_url(url).ok_or_else(|| {
                JsErrorBox::generic(format!("cannot resolve `{specifier}` from `{referrer}`"))
            })
        })
        .transpose()?;
    let base = parent.as_deref().unwrap_or_else(|| Path::new("/"));
    let resolved =
        thinpack_resolve::resolve_with_conditions(specifier, base, fs, &fs.conditions(true))
            .map_err(|error| {
                if let (thinpack_resolve::Error::NotFound, Some(parent_url)) = (&error, &parent_url)
                    && !specifier.starts_with('#')
                {
                    let is_path = matches!(specifier, "." | "..")
                        || specifier.starts_with("./")
                        || specifier.starts_with("../")
                        || specifier.starts_with('/');
                    let url = is_path.then(|| parent_url.join(specifier).ok()).flatten();
                    return module_not_found(fs, specifier, url.as_ref(), Some(parent_url));
                }
                JsErrorBox::generic(format!(
                    "cannot resolve `{specifier}` from `{referrer}`: {}",
                    error.message(|path| fs.js_path(path))
                ))
            })?;
    file_url(&resolved.path).ok_or_else(|| {
        JsErrorBox::generic(format!(
            "{} cannot be represented as a URL",
            fs.js_path(&resolved.path)
        ))
    })
}

fn module_not_found(
    fs: &ModuleFs,
    specifier: &str,
    url: Option<&ModuleSpecifier>,
    referrer: Option<&ModuleSpecifier>,
) -> JsErrorBox {
    let mut message = url.map_or_else(
        || format!("Cannot find package '{specifier}'"),
        |url| {
            let path = path_from_url(url).map_or_else(|| url.to_string(), |path| fs.js_path(&path));
            format!("Cannot find module '{path}'")
        },
    );
    if let Some(referrer) = referrer {
        let importer =
            path_from_url(referrer).map_or_else(|| referrer.to_string(), |path| fs.js_path(&path));
        write!(message, " imported from {importer}").unwrap();
    }
    JsErrorBox::from_err(LoaderError {
        code: "ERR_MODULE_NOT_FOUND",
        message,
        url: url.map(ToString::to_string),
    })
}

fn collect_reexports(
    fs: &ModuleFs,
    path: &Path,
    reexports: &[String],
    names: &mut BTreeSet<String>,
    visited: &mut BTreeSet<PathBuf>,
) {
    visited.insert(path.to_path_buf());

    for specifier in reexports {
        let Ok(resolved) =
            thinpack_resolve::resolve_with_conditions(specifier, path, fs, &fs.conditions(false))
        else {
            continue;
        };
        if !visited.insert(resolved.path.clone()) {
            continue;
        }
        if thinpack_resolve::format_of(&resolved.path, fs) != Ok(Format::CommonJs) {
            continue;
        }
        let Ok(contents) = fs.read(&resolved.path) else {
            continue;
        };
        let Ok(text) = String::from_utf8(contents) else {
            continue;
        };
        let analysis = thinpack_cjs_lexer::analyze(&text);
        names.extend(analysis.exports);
        collect_reexports(fs, &resolved.path, &analysis.reexports, names, visited);
    }
}

fn builtin_export_names(
    builtins: &BuiltinSources,
    source: &str,
    visited: &mut BTreeSet<String>,
) -> BTreeSet<String> {
    let analysis = thinpack_cjs_lexer::analyze(source);
    let mut names = analysis.exports;
    for specifier in analysis.reexports {
        if !visited.insert(specifier.clone()) {
            continue;
        }
        if let Some(inner) = builtins.get(&specifier) {
            names.extend(builtin_export_names(builtins, inner, visited));
        }
    }
    names
}
/// The format a URL loads as, without reading its source. A load hook receives this in
/// `context.format`, as Node determines the format before it calls the hook.
pub(crate) fn default_format(
    fs: &ModuleFs,
    builtins: &BuiltinSources,
    specifier: &ModuleSpecifier,
) -> Option<&'static str> {
    match specifier.scheme() {
        "node" => builtins.get(specifier.as_str()).map(|_| "builtin"),
        "file" => {
            let path = path_from_url(specifier)?;
            match thinpack_resolve::format_of(&path, fs) {
                Ok(Format::Json) => Some("json"),
                Ok(Format::Esm) => Some("module"),
                Ok(Format::CommonJs) => Some("commonjs"),
                Err(_) => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn default_source(
    fs: &ModuleFs,
    builtins: &BuiltinSources,
    specifier: &ModuleSpecifier,
    referrer: Option<&ModuleSpecifier>,
) -> Result<(&'static str, String), JsErrorBox> {
    match specifier.scheme() {
        "node" => builtins
            .get(specifier.as_str())
            .map(|source| ("builtin", source.to_owned()))
            .ok_or_else(|| {
                JsErrorBox::from_err(LoaderError {
                    code: "ERR_UNKNOWN_BUILTIN_MODULE",
                    message: crate::missing_builtin_message(specifier.as_str()),
                    url: None,
                })
            }),
        "file" => {
            let path = path_from_url(specifier)
                .ok_or_else(|| JsErrorBox::generic(format!("{specifier}: not a file URL")))?;
            if let Some(message) = crate::unstripped_typescript(fs, &path) {
                return Err(JsErrorBox::from_err(LoaderError {
                    code: "ERR_UNKNOWN_FILE_EXTENSION",
                    message,
                    url: Some(specifier.to_string()),
                }));
            }
            let bytes = fs.read(&path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    module_not_found(fs, specifier.as_str(), Some(specifier), referrer)
                } else {
                    JsErrorBox::generic(format!("{}: {error}", fs.js_path(&path)))
                }
            })?;
            let source = String::from_utf8(bytes).map_err(|_| {
                JsErrorBox::generic(format!("{}: module is not valid UTF-8", fs.js_path(&path)))
            })?;
            let format = match thinpack_resolve::format_of(&path, fs) {
                Ok(Format::Json) => "json",
                Ok(Format::Esm) => "module",
                Ok(Format::CommonJs) => "commonjs",
                Err(error) => {
                    return Err(JsErrorBox::generic(error.message(|path| fs.js_path(path))));
                }
            };
            Ok((format, source))
        }
        scheme => Err(JsErrorBox::generic(format!(
            "{specifier}: unsupported module scheme `{scheme}`"
        ))),
    }
}

fn load_from_hook(
    fs: &ModuleFs,
    builtins: &BuiltinSources,
    specifier: &ModuleSpecifier,
    hook_load: HookLoad,
    builtin_names: Option<&BTreeSet<String>>,
) -> Result<ModuleSource, JsErrorBox> {
    module_from_format(
        fs,
        builtins,
        specifier,
        &hook_load.format,
        hook_load.source,
        builtin_names,
    )
}

fn module_from_format(
    fs: &ModuleFs,
    builtins: &BuiltinSources,
    specifier: &ModuleSpecifier,
    format: &str,
    source: String,
    builtin_names: Option<&BTreeSet<String>>,
) -> Result<ModuleSource, JsErrorBox> {
    match format {
        "module" => Ok(module_source(specifier, ModuleType::JavaScript, source)),
        "json" => Ok(module_source(specifier, ModuleType::Json, source)),
        "commonjs" => {
            let path = path_from_url(specifier);
            let analysis = thinpack_cjs_lexer::analyze(&source);
            let mut names = analysis.exports;
            if let Some(path) = path.as_deref() {
                collect_reexports(
                    fs,
                    path,
                    &analysis.reexports,
                    &mut names,
                    &mut BTreeSet::new(),
                );
            }
            let id = path
                .as_deref()
                .map_or_else(|| specifier.as_str().to_owned(), |path| fs.js_path(path));
            let mut facade = format!(
                "const __exports__ = globalThis.__thinpack_require_source__({}, {}, {});\n",
                js_string_literal(&id),
                js_string_literal(specifier.as_str()),
                js_string_literal(&source),
            );
            append_named_exports(&mut facade, &names);
            Ok(module_source(specifier, ModuleType::JavaScript, facade))
        }
        "builtin" => {
            let mut names = builtin_export_names(builtins, &source, &mut BTreeSet::new());
            if let Some(runtime) = builtin_names {
                names.extend(runtime.iter().cloned());
            }
            Ok(module_source(
                specifier,
                ModuleType::JavaScript,
                facade(specifier.as_str(), &names),
            ))
        }
        other => Err(JsErrorBox::generic(format!(
            "{specifier}: unsupported module format `{other}`"
        ))),
    }
}

fn module_source(
    specifier: &ModuleSpecifier,
    module_type: ModuleType,
    source: String,
) -> ModuleSource {
    ModuleSource::new(
        module_type,
        ModuleSourceCode::String(source.into()),
        specifier,
        None,
    )
}

fn facade(target: &str, names: &std::collections::BTreeSet<String>) -> String {
    let mut source = format!(
        "const __exports__ = globalThis.__thinpack_require__({});\n",
        js_string_literal(target)
    );
    append_named_exports(&mut source, names);
    source
}

fn append_named_exports(source: &mut String, names: &std::collections::BTreeSet<String>) {
    for name in names {
        if !is_export_name(name) {
            continue;
        }
        let _ = writeln!(source, "export const {name} = __exports__.{name};");
    }
    source.push_str("export default __exports__;\n");
}

/// Whether `name` can be declared by the facade source.
///
/// The lexer and `Object.keys` return every detected key, including `default`, keys that
/// are not identifiers, and reserved words. `default` is the facade's own default export,
/// `__exports__` is its binding, and the rest cannot be declared with `export const`.
fn is_export_name(name: &str) -> bool {
    if matches!(name, "default" | "__exports__") || is_reserved(name) {
        return false;
    }
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(first) if is_identifier_start(first) => {}
        _ => return false,
    }
    bytes.all(is_identifier_part)
}

const fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

const fn is_identifier_part(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

/// Words that cannot appear as a binding name in a module (strict mode).
const RESERVED: &str = "arguments await break case catch class const continue debugger delete do \
else enum eval export extends false finally for function if implements import in instanceof \
interface let new null package private protected public return static super switch this throw \
true try typeof var void while with yield";

fn is_reserved(name: &str) -> bool {
    RESERVED.split_whitespace().any(|reserved| reserved == name)
}

#[must_use]
pub fn is_esm(path: &Path, fs: &ModuleFs) -> bool {
    matches!(thinpack_resolve::format_of(path, fs), Ok(Format::Esm))
}
