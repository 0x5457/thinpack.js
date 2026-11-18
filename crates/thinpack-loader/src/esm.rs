use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};

use deno_error::JsErrorBox;
use thinpack_core::BuiltinSources;
use thinpack_core::js_string_literal;
use thinpack_resolve::{FileSystem, Format};

use crate::ModuleFs;
use crate::cjs_exports;
use crate::ops::{file_url, path_from_url};

#[derive(Debug)]
struct HookLoad {
    format: String,
    source: String,
}

#[derive(Debug)]
pub struct EsmLoader {
    fs: ModuleFs,
    builtins: BuiltinSources,
    hook_loads: RefCell<HashMap<String, HookLoad>>,
    in_hook: Cell<bool>,
}

impl EsmLoader {
    #[must_use]
    pub fn new(fs: ModuleFs, builtins: BuiltinSources) -> Self {
        Self {
            fs,
            builtins,
            hook_loads: RefCell::new(HashMap::new()),
            in_hook: Cell::new(false),
        }
    }
}

impl ModuleLoader for EsmLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, JsErrorBox> {
        resolve(&self.fs, specifier, referrer).map_err(JsErrorBox::generic)
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

        if hook_reply.is_empty() {
            return self.resolve(specifier, referrer, kind);
        }

        let mut parts = hook_reply.splitn(3, '\0');
        let url = parts.next().unwrap_or_default();
        let format = parts.next();
        let source = parts.next();
        let specifier =
            ModuleSpecifier::parse(url).map_err(|error| JsErrorBox::generic(error.to_string()))?;
        if let (Some(format), Some(source)) = (format, source) {
            self.hook_loads.borrow_mut().insert(
                specifier.as_str().to_owned(),
                HookLoad {
                    format: format.to_owned(),
                    source: source.to_owned(),
                },
            );
        }
        Ok(specifier)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
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
            ));
        }
        ModuleLoadResponse::Sync(load(&self.fs, &self.builtins, module_specifier))
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
    deno_core::v8::tc_scope!(let scope, scope);
    let global = scope.get_current_context().global(scope);
    let name = deno_core::v8::String::new(scope, "__thinpack_esm_resolve__").unwrap();
    let bridge = global
        .get(scope, name.into())
        .unwrap_or_else(|| deno_core::v8::undefined(scope).into());
    let Ok(bridge) = deno_core::v8::Local::<deno_core::v8::Function>::try_from(bridge) else {
        return Err(JsErrorBox::generic(
            "the ESM loader hook bridge is not installed",
        ));
    };
    let args = [
        deno_core::v8::String::new(scope, specifier).unwrap().into(),
        deno_core::v8::String::new(scope, referrer).unwrap().into(),
        deno_core::v8::String::new(scope, attributes)
            .unwrap()
            .into(),
    ];
    let receiver = deno_core::v8::undefined(scope);
    let result = bridge.call(scope, receiver.into(), &args);
    if scope.has_caught() {
        let exception = scope.exception().unwrap();
        return Err(JsErrorBox::generic(exception_message(scope, exception)));
    }
    let Some(result) = result else {
        return Err(JsErrorBox::generic("ESM loader hook failed"));
    };
    Ok(result.to_rust_string_lossy(scope))
}

fn exception_message(
    scope: &mut deno_core::v8::PinScope<'_, '_>,
    exception: deno_core::v8::Local<'_, deno_core::v8::Value>,
) -> String {
    use deno_core::v8;
    let Some(object) = exception.to_object(scope) else {
        return exception.to_rust_string_lossy(scope);
    };
    let read = |scope: &mut v8::PinScope<'_, '_>, name: &str| {
        let key = v8::String::new(scope, name).unwrap();
        object
            .get(scope, key.into())
            .filter(|value| !value.is_undefined())
            .map(|value| value.to_rust_string_lossy(scope))
    };
    let message = read(scope, "message").unwrap_or_else(|| exception.to_rust_string_lossy(scope));
    match read(scope, "code") {
        Some(code) => format!("{code}: {message}"),
        None => message,
    }
}

pub(crate) fn resolve(
    fs: &ModuleFs,
    specifier: &str,
    referrer: &str,
) -> Result<ModuleSpecifier, String> {
    if specifier.starts_with("node:") {
        return ModuleSpecifier::parse(specifier).map_err(|error| error.to_string());
    }
    if specifier.starts_with("file:") {
        return ModuleSpecifier::parse(specifier).map_err(|error| error.to_string());
    }
    if thinpack_caps::module_for_builtin(specifier).is_some() {
        return ModuleSpecifier::parse(&format!("node:{specifier}"))
            .map_err(|error| error.to_string());
    }

    let parent = if referrer.is_empty() {
        None
    } else {
        let url = ModuleSpecifier::parse(referrer).map_err(|error| error.to_string())?;
        Some(
            path_from_url(&url)
                .ok_or_else(|| format!("cannot resolve `{specifier}` from `{referrer}`"))?,
        )
    };

    let base = parent.as_deref().unwrap_or_else(|| Path::new("/"));
    let resolved = thinpack_resolve::resolve_import(specifier, base, fs)
        .map_err(|error| format!("cannot resolve `{specifier}`: {error}"))?;
    let url = file_url(&resolved.path)
        .ok_or_else(|| format!("{} cannot be represented as a URL", resolved.path.display()))?;
    ModuleSpecifier::parse(&url).map_err(|error| error.to_string())
}

fn collect_reexports(
    fs: &ModuleFs,
    path: &Path,
    source: &str,
    names: &mut BTreeSet<String>,
    visited: &mut BTreeSet<PathBuf>,
) {
    visited.insert(path.to_path_buf());

    for specifier in cjs_exports::reexport_specifiers(source) {
        let Ok(resolved) = thinpack_resolve::resolve(&specifier, path, fs) else {
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
        names.extend(cjs_exports::export_names(&text));
        collect_reexports(fs, &resolved.path, &text, names, visited);
    }
}

fn builtin_export_names(
    builtins: &BuiltinSources,
    source: &str,
    visited: &mut BTreeSet<String>,
) -> BTreeSet<String> {
    let mut names = cjs_exports::export_names(source);
    for specifier in cjs_exports::reexport_specifiers(source) {
        if !visited.insert(specifier.clone()) {
            continue;
        }
        if let Some(inner) = builtins.get(&specifier) {
            names.extend(builtin_export_names(builtins, inner, visited));
        }
    }
    names
}
pub(crate) fn default_source(
    fs: &ModuleFs,
    builtins: &BuiltinSources,
    specifier: &ModuleSpecifier,
) -> Result<(&'static str, String), JsErrorBox> {
    match specifier.scheme() {
        "node" => builtins
            .get(specifier.as_str())
            .map(|source| ("builtin", source.to_owned()))
            .ok_or_else(|| JsErrorBox::generic(format!("No such built-in module: {specifier}"))),
        "file" => {
            let path = path_from_url(specifier)
                .ok_or_else(|| JsErrorBox::generic(format!("{specifier}: not a file URL")))?;
            let bytes = fs
                .read(&path)
                .map_err(|error| JsErrorBox::generic(format!("{}: {error}", path.display())))?;
            let source = String::from_utf8(bytes).map_err(|_| {
                JsErrorBox::generic(format!("{}: module is not valid UTF-8", path.display()))
            })?;
            let format = match thinpack_resolve::format_of(&path, fs) {
                Ok(Format::Json) => "json",
                Ok(Format::Esm) => "module",
                Ok(Format::CommonJs) => "commonjs",
                Err(error) => return Err(JsErrorBox::generic(error.to_string())),
            };
            Ok((format, source))
        }
        scheme => Err(JsErrorBox::generic(format!(
            "{specifier}: unsupported module scheme `{scheme}`"
        ))),
    }
}

fn load(
    fs: &ModuleFs,
    builtins: &BuiltinSources,
    specifier: &ModuleSpecifier,
) -> Result<ModuleSource, JsErrorBox> {
    let (format, source) = default_source(fs, builtins, specifier)?;
    module_from_format(fs, builtins, specifier, format, source)
}

fn load_from_hook(
    fs: &ModuleFs,
    builtins: &BuiltinSources,
    specifier: &ModuleSpecifier,
    hook_load: HookLoad,
) -> Result<ModuleSource, JsErrorBox> {
    module_from_format(fs, builtins, specifier, &hook_load.format, hook_load.source)
}

fn module_from_format(
    fs: &ModuleFs,
    builtins: &BuiltinSources,
    specifier: &ModuleSpecifier,
    format: &str,
    source: String,
) -> Result<ModuleSource, JsErrorBox> {
    match format {
        "module" => Ok(module_source(specifier, ModuleType::JavaScript, source)),
        "json" => Ok(module_source(specifier, ModuleType::Json, source)),
        "commonjs" => {
            let path = path_from_url(specifier);
            let mut names = cjs_exports::export_names(&source);
            if let Some(path) = path.as_deref() {
                collect_reexports(fs, path, &source, &mut names, &mut BTreeSet::new());
            }
            let mut facade = format!(
                "const __exports__ = globalThis.__thinpack_require_source__({}, {}, {});\n",
                js_string_literal(&path.as_deref().map_or_else(
                    || specifier.as_str().to_owned(),
                    |path| path.to_string_lossy().into_owned(),
                )),
                js_string_literal(specifier.as_str()),
                js_string_literal(&source),
            );
            append_named_exports(&mut facade, &names);
            Ok(module_source(specifier, ModuleType::JavaScript, facade))
        }
        "builtin" => {
            let names = builtin_export_names(builtins, &source, &mut BTreeSet::new());
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
        if name == "__exports__" {
            continue;
        }
        let _ = writeln!(source, "export const {name} = __exports__.{name};");
    }
    source.push_str("export default __exports__;\n");
}

#[must_use]
pub fn is_esm(path: &Path, fs: &ModuleFs) -> bool {
    matches!(thinpack_resolve::format_of(path, fs), Ok(Format::Esm))
}
