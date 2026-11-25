use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::json::{self, Json};
use crate::{Conditions, Error, FileSystem};

#[derive(Debug, Default)]
pub(crate) struct Manifest {
    pub name: Option<String>,
    pub main: Option<String>,
    pub module_type: Option<bool>,
    pub exports: Option<Json>,
    pub imports: Option<Json>,
    bins: BTreeMap<String, String>,
    optional: Vec<String>,
}

impl Manifest {
    pub(crate) fn read(path: &Path, fs: &impl FileSystem) -> Result<Self, Error> {
        if !fs.is_file(path) {
            return Ok(Self::default());
        }
        let text = fs.read(path).map_err(|error| Error::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let text = String::from_utf8_lossy(&text);
        let value = json::parse(&text).map_err(|message| Error::InvalidPackage {
            path: path.to_path_buf(),
            message,
        })?;

        let module_type = match value.get("type") {
            None => None,
            Some(Json::Str(kind)) if kind == "module" => Some(true),
            Some(Json::Str(kind)) if kind == "commonjs" => Some(false),
            Some(other) => {
                return Err(Error::InvalidPackage {
                    path: path.to_path_buf(),
                    message: format!("`type` must be \"module\" or \"commonjs\", got {other:?}"),
                });
            }
        };

        let name = value.get("name").and_then(Json::as_str).map(str::to_owned);
        Ok(Self {
            name: name.clone(),
            main: value.get("main").and_then(Json::as_str).map(str::to_owned),
            module_type,
            exports: value.get("exports").cloned(),
            imports: value.get("imports").cloned(),
            bins: bin_targets(&value, name.as_deref()),
            optional: optional_names(&value),
        })
    }
    pub(crate) fn is_module(&self) -> bool {
        self.module_type == Some(true)
    }
}

fn bin_targets(value: &Json, name: Option<&str>) -> BTreeMap<String, String> {
    match value.get("bin") {
        Some(Json::Str(target)) => name
            .map(|name| BTreeMap::from([(name.to_owned(), target.clone())]))
            .unwrap_or_default(),
        Some(Json::Object(entries)) => entries
            .iter()
            .filter_map(|(name, target)| {
                target
                    .as_str()
                    .map(|target| (name.clone(), target.to_owned()))
            })
            .collect(),
        _ => BTreeMap::new(),
    }
}

fn optional_names(value: &Json) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(Json::Object(entries)) = value.get("optionalDependencies") {
        names.extend(entries.iter().map(|(name, _)| name.clone()));
    }
    if let Some(Json::Object(entries)) = value.get("peerDependenciesMeta") {
        for (name, meta) in entries {
            if matches!(meta.get("optional"), Some(Json::Bool(true))) {
                names.push(name.clone());
            }
        }
    }
    names
}

/// The executable target declared as `name` by the package manifest at `manifest`.
///
/// The target is resolved against the package directory; a target that leaves the
/// package is ignored. The caller decides whether the target is code or data.
///
/// # Errors
///
/// Returns the manifest read and parse errors.
pub fn declared_bin(
    manifest: &Path,
    name: &str,
    fs: &impl FileSystem,
) -> Result<Option<PathBuf>, Error> {
    let Some(directory) = manifest.parent() else {
        return Ok(None);
    };
    let manifest = Manifest::read(manifest, fs)?;
    let Some(target) = manifest.bins.get(name) else {
        return Ok(None);
    };
    let path = crate::normalize(&directory.join(target));
    Ok(path.starts_with(directory).then_some(path))
}

/// The optional dependency names declared by the nearest package scope above `path`.
///
/// `optionalDependencies` and optional `peerDependenciesMeta` both count. Read once per
/// analyzed file so a resolved package outside the project root can still be classified
/// as optional.
pub fn optional_dependencies(path: &Path, fs: &impl FileSystem) -> Vec<String> {
    let mut directory = path.parent();
    while let Some(current) = directory {
        let manifest_path = current.join("package.json");
        if fs.is_file(&manifest_path) {
            return Manifest::read(&manifest_path, fs)
                .map_or_else(|_| Vec::new(), |manifest| manifest.optional);
        }
        directory = current.parent();
    }
    Vec::new()
}

#[must_use]
pub fn is_optional_dependency(specifier: &str, optional: &[String]) -> bool {
    let name = package_name(specifier);
    optional.iter().any(|entry| entry == name)
}

pub fn declares_optional(specifier: &str, path: &Path, fs: &impl FileSystem) -> bool {
    is_optional_dependency(specifier, &optional_dependencies(path, fs))
}

#[must_use]
pub fn package_name(specifier: &str) -> &str {
    specifier.strip_prefix('@').map_or_else(
        || {
            specifier
                .find('/')
                .map_or(specifier, |index| &specifier[..index])
        },
        |rest| {
            rest.find('/').map_or(specifier, |scope_end| {
                rest[scope_end + 1..]
                    .find('/')
                    .map_or(specifier, |name_end| {
                        &specifier[..=scope_end + name_end + 1]
                    })
            })
        },
    )
}

pub(crate) fn resolve_subpath(
    exports: &Json,
    subpath: &str,
    conditions: &Conditions,
) -> Option<String> {
    if let Some(map) = exports.subpath_map() {
        let key = if subpath.is_empty() {
            ".".to_owned()
        } else {
            format!("./{subpath}")
        };
        if let Some(target) = lookup_exact(map, &key) {
            return resolve_target(target, conditions);
        }
        return lookup_pattern(map, &key, conditions);
    }

    if !subpath.is_empty() {
        return None;
    }
    resolve_target(exports, conditions)
}

fn lookup_exact<'a>(map: &'a [(String, Json)], key: &str) -> Option<&'a Json> {
    map.iter()
        .find(|(pattern, _)| pattern == key)
        .map(|(_, target)| target)
}

fn lookup_pattern(map: &[(String, Json)], key: &str, conditions: &Conditions) -> Option<String> {
    lookup_star(map, key, &|target| resolve_target(target, conditions))
}

fn lookup_star(
    map: &[(String, Json)],
    key: &str,
    resolve: &dyn Fn(&Json) -> Option<String>,
) -> Option<String> {
    let mut best: Option<(&str, &str, &Json)> = None;
    for (pattern, target) in map {
        let Some((prefix, suffix)) = pattern.split_once('*') else {
            continue;
        };
        if !key.starts_with(prefix) || !key.ends_with(suffix) {
            continue;
        }
        if key.len() < prefix.len() + suffix.len() {
            continue;
        }
        if best.is_none_or(|(best_prefix, _, _)| prefix.len() > best_prefix.len()) {
            best = Some((prefix, suffix, target));
        }
    }
    let (prefix, suffix, target) = best?;
    let star = &key[prefix.len()..key.len() - suffix.len()];
    resolve(target).map(|path| path.replace('*', star))
}

fn resolve_target(value: &Json, conditions: &Conditions) -> Option<String> {
    match value {
        Json::Str(target) => strip_relative(target),
        Json::Array(targets) => targets
            .iter()
            .find_map(|target| resolve_target(target, conditions)),
        Json::Object(entries) => entries.iter().find_map(|(condition, target)| {
            conditions
                .accepts(condition)
                .then(|| resolve_target(target, conditions))
                .flatten()
        }),
        _ => None,
    }
}

pub(crate) fn resolve_import(
    imports: &Json,
    specifier: &str,
    conditions: &Conditions,
) -> Option<String> {
    let map = imports.object_map()?;
    if let Some(target) = lookup_exact(map, specifier) {
        return import_target(target, conditions);
    }
    lookup_import_pattern(map, specifier, conditions)
}

fn import_target(value: &Json, conditions: &Conditions) -> Option<String> {
    match value {
        Json::Str(target) => Some(target.clone()),
        Json::Array(targets) => targets
            .iter()
            .find_map(|target| import_target(target, conditions)),
        Json::Object(entries) => entries.iter().find_map(|(condition, target)| {
            conditions
                .accepts(condition)
                .then(|| import_target(target, conditions))
                .flatten()
        }),
        _ => None,
    }
}

fn lookup_import_pattern(
    map: &[(String, Json)],
    key: &str,
    conditions: &Conditions,
) -> Option<String> {
    lookup_star(map, key, &|target| import_target(target, conditions))
}

fn strip_relative(target: &str) -> Option<String> {
    let relative = target.strip_prefix("./")?;
    if relative.split('/').any(|segment| segment == "..") {
        return None;
    }
    Some(relative.to_owned())
}

/// The `type` of the nearest package scope.
///
/// `Some(true)` is `module`, `Some(false)` is `commonjs`, and `None` means the field
/// is absent or there is no package.json. Node's syntax detection only applies to the
/// `None` case.
///
/// # Errors
///
/// Returns the manifest read and parse errors from the package scope walk.
pub fn package_type(path: &Path, fs: &impl FileSystem) -> Result<Option<bool>, Error> {
    let mut directory = path.parent();
    while let Some(current) = directory {
        if current
            .file_name()
            .is_some_and(|name| name == "node_modules")
        {
            return Ok(None);
        }
        let manifest_path = current.join("package.json");
        if fs.is_file(&manifest_path) {
            return Ok(Manifest::read(&manifest_path, fs)?.module_type);
        }
        directory = current.parent();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exports(source: &str) -> Json {
        json::parse(source).unwrap()
    }

    #[test]
    fn resolves_root_string() {
        let value = exports(r#""./index.js""#);
        assert_eq!(
            resolve_subpath(&value, "", &Conditions::require()).as_deref(),
            Some("index.js")
        );
        assert_eq!(resolve_subpath(&value, "sub", &Conditions::require()), None);
    }

    #[test]
    fn follows_condition_order() {
        let value = exports(r#"{"import": "./esm.js", "default": "./cjs.js"}"#);
        assert_eq!(
            resolve_subpath(&value, "", &Conditions::require()).as_deref(),
            Some("cjs.js")
        );
    }

    #[test]
    fn resolves_subpath_map_and_patterns() {
        let value = exports(r#"{"./a": "./a.js", "./lib/*": "./src/*.js"}"#);
        assert_eq!(
            resolve_subpath(&value, "a", &Conditions::require()).as_deref(),
            Some("a.js")
        );
        assert_eq!(
            resolve_subpath(&value, "lib/x", &Conditions::require()).as_deref(),
            Some("src/x.js")
        );
        assert_eq!(
            resolve_subpath(&value, "other", &Conditions::require()),
            None
        );
    }

    #[test]
    fn rejects_escaping_targets() {
        let value = exports(r#""../outside.js""#);
        assert_eq!(resolve_subpath(&value, "", &Conditions::require()), None);
    }
}
