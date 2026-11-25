mod json;

mod package;

use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

use package::Manifest;

pub use package::{
    declares_optional, is_optional_dependency, optional_dependencies, package_name, package_type,
};

pub trait FileSystem {
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;

    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when the path cannot be read.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// The module format recorded in the payload for this file, when packing
    /// detected one (a typeless `.js` written as an ES module).
    fn format_override(&self, _path: &Path) -> Option<Format> {
        None
    }

    /// Custom `exports`/`imports` conditions recorded in the payload.
    fn custom_conditions(&self) -> &[String] {
        &[]
    }
}

#[derive(Debug)]
pub struct HostFs;

impl FileSystem for HostFs {
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    CommonJs,
    Json,
    Esm,
}

/// The conditions a package `exports`/`imports` object is resolved with: the
/// caller kind (`import` or `require`) plus any `--conditions` names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conditions {
    pub import: bool,
    pub custom: Vec<String>,
}

impl Conditions {
    #[must_use]
    pub fn require() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn import() -> Self {
        Self {
            import: true,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_custom(mut self, custom: impl IntoIterator<Item = String>) -> Self {
        self.custom = custom.into_iter().collect();
        self.custom.sort_unstable();
        self.custom.dedup();
        self
    }

    #[must_use]
    pub fn accepts(&self, condition: &str) -> bool {
        match condition {
            "node" | "default" => true,
            "import" => self.import,
            "require" => !self.import,
            custom => self.custom.iter().any(|candidate| candidate == custom),
        }
    }
}

impl Format {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::CommonJs => "commonjs",
            Self::Json => "json",
            Self::Esm => "esm",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "commonjs" => Some(Self::CommonJs),
            "json" => Some(Self::Json),
            "esm" => Some(Self::Esm),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub format: Format,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    NotFound,
    NotExported { subpath: String },
    Unsupported { path: PathBuf },
    InvalidPackage { path: PathBuf, message: String },
    Io { path: PathBuf, message: String },
}

impl Error {
    /// Renders the error with paths formatted by `render`.
    ///
    /// Callers that hand paths to a layer with its own path space — the loader
    /// names packed payload paths POSIX on every platform — pass their own
    /// formatter; [`Display`](fmt::Display) renders host paths.
    pub fn message(&self, render: impl Fn(&Path) -> String) -> String {
        match self {
            Self::NotFound => "module not found".to_owned(),
            Self::NotExported { subpath } => format!("package subpath '{subpath}' is not exported"),
            Self::Unsupported { path } => {
                format!("{} is not a supported module type", render(path))
            }
            Self::InvalidPackage { path, message } => {
                format!("invalid {}: {message}", render(path))
            }
            Self::Io { path, message } => format!("{}: {message}", render(path)),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message(|path| path.display().to_string()))
    }
}

impl std::error::Error for Error {}

/// # Errors
///
/// Returns [`Error::NotFound`] when the specifier cannot be resolved, and the
/// parse, export-map and I/O errors reported by the walked `package.json`
/// manifests.
pub fn resolve(specifier: &str, parent: &Path, fs: &impl FileSystem) -> Result<Resolved, Error> {
    resolve_with_conditions(specifier, parent, fs, &Conditions::require())
}

/// [`resolve`] with explicit package conditions.
///
/// # Errors
///
/// Returns the same errors as [`resolve`].
pub fn resolve_with_conditions(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
    conditions: &Conditions,
) -> Result<Resolved, Error> {
    let path = resolve_with(specifier, parent, fs, conditions)?;
    let format = format_of(&path, fs)?;
    Ok(Resolved { path, format })
}

/// # Errors
///
/// Returns the same errors as [`resolve`], resolved with `import` conditions
/// taking precedence over `require`.
pub fn resolve_import(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
) -> Result<Resolved, Error> {
    resolve_with_conditions(specifier, parent, fs, &Conditions::import())
}

/// # Errors
///
/// Returns the same errors as [`resolve`] except the module format: the
/// `package.json` ancestor walk that classifies a generic `.js` file is
/// skipped, for callers that only need the location. Native addons are still
/// rejected.
pub fn resolve_path(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
) -> Result<PathBuf, Error> {
    resolve_path_with_conditions(specifier, parent, fs, &Conditions::require())
}

/// [`resolve_path`] with explicit package conditions.
///
/// # Errors
///
/// Returns the same errors as [`resolve_path`].
pub fn resolve_path_with_conditions(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
    conditions: &Conditions,
) -> Result<PathBuf, Error> {
    let path = resolve_with(specifier, parent, fs, conditions)?;
    reject_native_addon(&path)?;
    Ok(path)
}

/// # Errors
///
/// Returns the same errors as [`resolve_path`], resolved with `import`
/// conditions taking precedence over `require`.
pub fn resolve_import_path(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
) -> Result<PathBuf, Error> {
    resolve_path_with_conditions(specifier, parent, fs, &Conditions::import())
}

fn reject_native_addon(path: &Path) -> Result<(), Error> {
    if path
        .extension()
        .is_some_and(|extension| extension == "node")
    {
        return Err(Error::Unsupported {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn resolve_with(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
    conditions: &Conditions,
) -> Result<PathBuf, Error> {
    if specifier.starts_with('#') {
        return resolve_package_import(specifier, parent, fs, conditions);
    }
    // A leading `/` is rooted on Unix and virtual `/app` paths on Windows are
    // not host-absolute (they have no drive), but they are still absolute
    // specifiers, not package names.
    if Path::new(specifier).is_absolute() || specifier.starts_with('/') {
        return load_path(&normalize(Path::new(specifier)), fs);
    }
    if matches!(specifier, "." | "..")
        || specifier.starts_with("./")
        || specifier.starts_with("../")
    {
        let base = parent.parent().unwrap_or_else(|| Path::new("/"));
        return load_path(&normalize(&base.join(specifier)), fs);
    }
    load_package(specifier, parent, fs, conditions)
}

fn resolve_package_import(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
    conditions: &Conditions,
) -> Result<PathBuf, Error> {
    let mut directory = parent.parent().unwrap_or_else(|| Path::new("/"));
    loop {
        let manifest_path = directory.join("package.json");
        if fs.is_file(&manifest_path) {
            let manifest = Manifest::read(&manifest_path, fs)?;
            let Some(imports) = manifest.imports else {
                return Err(Error::NotFound);
            };
            let target =
                package::resolve_import(&imports, specifier, conditions).ok_or(Error::NotFound)?;
            if target.starts_with("./") {
                return load_path(&normalize(&directory.join(target)), fs);
            }
            if target.starts_with("../") {
                return Err(Error::NotFound);
            }
            return resolve_with(&target, &directory.join("index.js"), fs, conditions);
        }
        match directory.parent() {
            Some(next) => directory = next,
            None => return Err(Error::NotFound),
        }
    }
}

fn load_package(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
    conditions: &Conditions,
) -> Result<PathBuf, Error> {
    let (name, subpath) = split_package(specifier);
    let base = parent.parent().unwrap_or_else(|| Path::new("/"));
    if let Some(directory) = package_self_reference(name, base, fs)? {
        let manifest = Manifest::read(&directory.join("package.json"), fs)?;
        return resolve_package(&directory, &manifest, subpath, conditions, fs);
    }
    let directory = find_package(name, base, fs).ok_or(Error::NotFound)?;
    let manifest = Manifest::read(&directory.join("package.json"), fs)?;
    resolve_package(&directory, &manifest, subpath, conditions, fs)
}

/// Node's package self-reference: the nearest package scope may import its own
/// `name` through its `exports` map before `node_modules` is searched.
fn package_self_reference(
    name: &str,
    base: &Path,
    fs: &impl FileSystem,
) -> Result<Option<PathBuf>, Error> {
    for ancestor in base.ancestors() {
        if ancestor
            .file_name()
            .is_some_and(|entry| entry == "node_modules")
        {
            return Ok(None);
        }
        let manifest = ancestor.join("package.json");
        if fs.is_file(&manifest) {
            let manifest = Manifest::read(&manifest, fs)?;
            return Ok(
                (manifest.name.as_deref() == Some(name) && manifest.exports.is_some())
                    .then(|| ancestor.to_path_buf()),
            );
        }
    }
    Ok(None)
}

fn resolve_package(
    directory: &Path,
    manifest: &Manifest,
    subpath: &str,
    conditions: &Conditions,
    fs: &impl FileSystem,
) -> Result<PathBuf, Error> {
    if let Some(exports) = &manifest.exports {
        let target = package::resolve_subpath(exports, subpath, conditions).ok_or_else(|| {
            Error::NotExported {
                subpath: subpath.to_owned(),
            }
        })?;
        let path = directory.join(target);
        return load_file(&path, fs).ok_or(Error::NotFound);
    }

    if subpath.is_empty() {
        if let Some(main) = &manifest.main
            && let Some(resolved) = load_file_or_directory(&directory.join(main), fs)?
        {
            return Ok(resolved);
        }
        return load_index(directory, fs).ok_or(Error::NotFound);
    }

    load_path(&directory.join(subpath), fs)
}

fn find_package(name: &str, base: &Path, fs: &impl FileSystem) -> Option<PathBuf> {
    node_module_paths(base).into_iter().find_map(|directory| {
        let candidate = directory.join(name);
        fs.is_dir(&candidate).then_some(candidate)
    })
}

fn split_package(specifier: &str) -> (&str, &str) {
    let name_end = if specifier.starts_with('@') {
        specifier.find('/').map_or(specifier.len(), |scope| {
            specifier[scope + 1..]
                .find('/')
                .map_or(specifier.len(), |name| scope + 1 + name)
        })
    } else {
        specifier.find('/').unwrap_or(specifier.len())
    };

    (
        &specifier[..name_end],
        specifier[name_end..].trim_start_matches('/'),
    )
}

fn load_path(path: &Path, fs: &impl FileSystem) -> Result<PathBuf, Error> {
    load_file_or_directory(path, fs)?.ok_or(Error::NotFound)
}

fn load_file_or_directory(path: &Path, fs: &impl FileSystem) -> Result<Option<PathBuf>, Error> {
    // `main` may point outside its directory; the runtime's virtual file system only
    // knows normalized paths, so normalize before looking the joined path up.
    let path = normalize(path);
    if let Some(resolved) = load_file(&path, fs) {
        return Ok(Some(resolved));
    }
    if fs.is_dir(&path) {
        return load_directory(&path, fs);
    }
    Ok(None)
}

fn load_file(path: &Path, fs: &impl FileSystem) -> Option<PathBuf> {
    // Export targets and `main` may carry `.` segments, and the payload's virtual
    // file system only knows normalized paths.
    let path = normalize(path);
    if fs.is_file(&path) {
        return Some(path);
    }

    for extension in ["js", "json", "cjs", "mjs"] {
        let candidate = append_extension(&path, extension);
        if fs.is_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn load_directory(directory: &Path, fs: &impl FileSystem) -> Result<Option<PathBuf>, Error> {
    let manifest = Manifest::read(&directory.join("package.json"), fs)?;
    if let Some(main) = &manifest.main
        && let Some(resolved) = load_file_or_directory(&directory.join(main), fs)?
    {
        return Ok(Some(resolved));
    }
    Ok(load_index(directory, fs))
}

fn load_index(directory: &Path, fs: &impl FileSystem) -> Option<PathBuf> {
    for name in ["index.js", "index.json"] {
        let path = directory.join(name);
        if fs.is_file(&path) {
            return Some(normalize(&path));
        }
    }
    None
}

/// Whether the loader executes this file as JavaScript.
///
/// TypeScript counts: packing strips its types, and the loader never sees a type
/// annotation. `.tsx` counts only so an explicit include reaches the analyzer's
/// unsupported-syntax error instead of being packed as an asset that cannot run.
/// Declaration files are data, not modules, so they stay out; JSON and native
/// addons are separate formats.
#[must_use]
pub fn is_javascript(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "cjs" | "mjs" | "ts" | "cts" | "mts" | "tsx")
    ) && !is_declaration(path)
}

/// `.d.ts`, `.d.mts` and `.d.cts` describe types for a caller; they are not
/// executable modules even though their last extension says TypeScript.
#[must_use]
pub fn is_declaration(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts")
}

/// Whether the path names TypeScript, which a packed artifact carries only after
/// packing stripped its types into JavaScript.
#[must_use]
pub fn is_typescript(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("ts" | "cts" | "mts" | "tsx")
    )
}

/// Whether any component of the path is a `node_modules` directory. Node keeps
/// type stripping out of installed packages, and packing follows that rule.
#[must_use]
pub fn is_under_node_modules(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "node_modules")
}

/// # Errors
///
/// Returns [`Error::Unsupported`] for native addons and the manifest errors
/// raised while looking for the nearest `package.json`.
pub fn format_of(path: &Path, fs: &impl FileSystem) -> Result<Format, Error> {
    if let Some(format) = fs.format_override(path) {
        return Ok(format);
    }
    // TypeScript follows Node's extension rules: `.mts` and `.cts` fix the
    // format, and `.ts` reads the package scope like `.js`.
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => Ok(Format::Json),
        Some("mjs" | "mts") => Ok(Format::Esm),
        Some("cjs" | "cts") => Ok(Format::CommonJs),
        Some("node") => Err(Error::Unsupported {
            path: path.to_path_buf(),
        }),
        _ => {
            let directory = path.parent().unwrap_or_else(|| Path::new("/"));
            if package_is_module(directory, fs)? {
                Ok(Format::Esm)
            } else {
                Ok(Format::CommonJs)
            }
        }
    }
}

fn package_is_module(directory: &Path, fs: &impl FileSystem) -> Result<bool, Error> {
    // Node's getPackageScopeConfig: the nearest package.json decides, even when it
    // has no `type`, and the walk stops at a node_modules boundary. The filesystem
    // root is still a package scope, as it is in Node.
    for ancestor in directory.ancestors() {
        if ancestor
            .file_name()
            .is_some_and(|name| name == "node_modules")
        {
            return Ok(false);
        }
        let manifest = ancestor.join("package.json");
        if fs.is_file(&manifest) {
            return Ok(Manifest::read(&manifest, fs)?.is_module());
        }
    }
    Ok(false)
}

fn append_extension(path: &Path, extension: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(extension);
    PathBuf::from(name)
}

/// Returns the `node_modules` lookup directories, nearest first.
///
/// A directory already named `node_modules` does not gain a nested entry,
/// matching Node's `Module._nodeModulePaths`.
#[must_use]
pub fn node_module_paths(from: &Path) -> Vec<PathBuf> {
    let from = normalize(from);
    from.ancestors()
        .filter(|directory| !directory.as_os_str().is_empty())
        .filter(|directory| {
            directory
                .file_name()
                .is_none_or(|name| name != "node_modules")
        })
        .map(|directory| directory.join("node_modules"))
        .collect()
}

/// Lexically removes `.` components and resolves `..`, the same semantics the
/// runtime's virtual-path lookup applies when it resolves a specifier.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    struct MemoryFs {
        files: BTreeMap<PathBuf, String>,
        directories: BTreeSet<PathBuf>,
    }

    impl MemoryFs {
        fn new(files: &[(&str, &str)]) -> Self {
            let mut fs = Self {
                files: BTreeMap::new(),
                directories: BTreeSet::new(),
            };
            for (path, contents) in files {
                let path = PathBuf::from(path);
                let mut ancestor = path.parent();
                while let Some(directory) = ancestor {
                    if !fs.directories.insert(directory.to_path_buf()) {
                        break;
                    }
                    ancestor = directory.parent();
                }
                fs.files.insert(path, (*contents).to_owned());
            }
            fs
        }
    }

    impl FileSystem for MemoryFs {
        fn is_file(&self, path: &Path) -> bool {
            self.files.contains_key(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.directories.contains(path)
        }

        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.files
                .get(path)
                .map(|contents| contents.clone().into_bytes())
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    fn resolve_from(fs: &MemoryFs, specifier: &str, parent: &str) -> Result<Resolved, Error> {
        resolve(specifier, Path::new(parent), fs)
    }

    #[test]
    fn resolves_relative_files_and_directories() {
        let fs = MemoryFs::new(&[
            ("/app/src/main.js", ""),
            ("/app/src/util.js", ""),
            ("/app/src/lib/index.js", ""),
        ]);
        let util = resolve_from(&fs, "./util", "/app/src/main.js").unwrap();
        assert_eq!(util.path, PathBuf::from("/app/src/util.js"));
        let lib = resolve_from(&fs, "./lib", "/app/src/main.js").unwrap();
        assert_eq!(lib.path, PathBuf::from("/app/src/lib/index.js"));
    }

    #[test]
    fn resolves_an_exports_target_with_dot_segments() {
        let fs = MemoryFs::new(&[
            (
                "/app/node_modules/pkg/package.json",
                r#"{"exports": "././index.js"}"#,
            ),
            ("/app/node_modules/pkg/index.js", ""),
        ]);
        assert_eq!(
            resolve_from(&fs, "pkg", "/app/main.js").unwrap().path,
            PathBuf::from("/app/node_modules/pkg/index.js")
        );
    }

    #[test]
    fn resolves_a_directory_main_outside_the_directory() {
        let fs = MemoryFs::new(&[
            ("/app/lib/package.json", r#"{"main": "../shared/entry.js"}"#),
            ("/app/shared/entry.js", ""),
        ]);
        assert_eq!(
            resolve_from(&fs, "./lib", "/app/main.js").unwrap().path,
            PathBuf::from("/app/shared/entry.js")
        );
    }

    struct OverrideFs(MemoryFs);

    impl FileSystem for OverrideFs {
        fn is_file(&self, path: &Path) -> bool {
            self.0.is_file(path)
        }
        fn is_dir(&self, path: &Path) -> bool {
            self.0.is_dir(path)
        }
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.0.read(path)
        }
        fn format_override(&self, _path: &Path) -> Option<Format> {
            Some(Format::Esm)
        }
    }

    #[test]
    fn a_format_override_wins_over_the_extension() {
        let fs = OverrideFs(MemoryFs::new(&[("/app/main.js", "")]));
        assert_eq!(format_of(Path::new("/app/main.js"), &fs), Ok(Format::Esm));
    }

    #[test]
    fn rejects_an_invalid_package_type() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type":"modules"}"#),
            ("/app/main.js", ""),
        ]);
        let error = resolve_from(&fs, "./main.js", "/app/other.js").unwrap_err();
        assert!(
            matches!(error, Error::InvalidPackage { .. }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn resolves_custom_export_conditions() {
        let fs = MemoryFs::new(&[
            (
                "/app/node_modules/pkg/package.json",
                r#"{"exports":{".":{"custom":"./custom.js","default":"./default.js"}}}"#,
            ),
            ("/app/node_modules/pkg/custom.js", ""),
            ("/app/node_modules/pkg/default.js", ""),
        ]);
        let conditions = Conditions::require().with_custom(["custom".to_owned()]);
        assert_eq!(
            resolve_with_conditions("pkg", Path::new("/app/main.js"), &fs, &conditions)
                .unwrap()
                .path,
            PathBuf::from("/app/node_modules/pkg/custom.js")
        );
        assert_eq!(
            resolve("pkg", Path::new("/app/main.js"), &fs).unwrap().path,
            PathBuf::from("/app/node_modules/pkg/default.js")
        );
    }

    #[test]
    fn package_imports_honour_the_caller_kind() {
        let fs = MemoryFs::new(&[
            (
                "/app/package.json",
                r##"{"imports":{"#x":{"import":"./esm.js","require":"./cjs.js"}}}"##,
            ),
            ("/app/esm.js", ""),
            ("/app/cjs.js", ""),
        ]);
        assert_eq!(
            resolve_with_conditions("#x", Path::new("/app/main.mjs"), &fs, &Conditions::import())
                .unwrap()
                .path,
            PathBuf::from("/app/esm.js")
        );
        assert_eq!(
            resolve_with_conditions(
                "#x",
                Path::new("/app/main.cjs"),
                &fs,
                &Conditions::require()
            )
            .unwrap()
            .path,
            PathBuf::from("/app/cjs.js")
        );
    }

    #[test]
    fn resolves_a_package_self_reference() {
        let fs = MemoryFs::new(&[
            (
                "/app/package.json",
                r#"{"name":"self-ref-demo","type":"module","exports":{".":"./index.mjs","./x":"./x.mjs"}}"#,
            ),
            ("/app/index.mjs", ""),
            ("/app/x.mjs", ""),
            ("/app/bin/cli.mjs", ""),
        ]);
        assert_eq!(
            resolve_from(&fs, "self-ref-demo", "/app/bin/cli.mjs")
                .unwrap()
                .path,
            PathBuf::from("/app/index.mjs")
        );
        assert_eq!(
            resolve_from(&fs, "self-ref-demo/x", "/app/bin/cli.mjs")
                .unwrap()
                .path,
            PathBuf::from("/app/x.mjs")
        );
    }

    #[test]
    fn resolves_package_imports() {
        let fs = MemoryFs::new(&[
            ("/app/main.mjs", ""),
            ("/app/package.json", r##"{"imports":{"#x":"./x.js"}}"##),
            ("/app/x.js", ""),
        ]);
        let resolved = resolve_from(&fs, "#x", "/app/main.mjs").unwrap();
        assert_eq!(resolved.path, PathBuf::from("/app/x.js"));
    }

    #[test]
    fn walks_up_to_node_modules() {
        let fs = MemoryFs::new(&[
            ("/app/src/deep/main.js", ""),
            ("/app/node_modules/pkg/index.js", ""),
        ]);
        let resolved = resolve_from(&fs, "pkg", "/app/src/deep/main.js").unwrap();
        assert_eq!(
            resolved.path,
            PathBuf::from("/app/node_modules/pkg/index.js")
        );
    }

    #[test]
    fn resolves_bare_dot_and_dotdot_as_directories() {
        let fs = MemoryFs::new(&[
            (
                "/app/node_modules/pkg/package.json",
                r#"{"main": "main.js"}"#,
            ),
            ("/app/node_modules/pkg/main.js", ""),
            ("/app/node_modules/pkg/bin/tool.js", ""),
            ("/app/node_modules/pkg/bin/index.js", ""),
        ]);
        let parent = "/app/node_modules/pkg/bin/tool.js";
        assert_eq!(
            resolve_from(&fs, "..", parent).unwrap().path,
            PathBuf::from("/app/node_modules/pkg/main.js")
        );
        assert_eq!(
            resolve_from(&fs, ".", parent).unwrap().path,
            PathBuf::from("/app/node_modules/pkg/bin/index.js")
        );
    }

    #[test]
    fn reports_optional_dependencies_from_package_json() {
        let fs = MemoryFs::new(&[
            (
                "/app/node_modules/pkg/package.json",
                r#"{"optionalDependencies": {"native-helper": "1"}, "peerDependenciesMeta": {"jiti": {"optional": true}}}"#,
            ),
            ("/app/node_modules/pkg/lib/x.js", ""),
        ]);
        let path = Path::new("/app/node_modules/pkg/lib/x.js");
        assert!(declares_optional("native-helper", path, &fs));
        assert!(declares_optional("jiti/sub", path, &fs));
        assert!(!declares_optional("lodash", path, &fs));
    }

    #[test]
    fn honours_package_main_and_exports() {
        let fs = MemoryFs::new(&[
            ("/app/main.js", ""),
            (
                "/app/node_modules/with-main/package.json",
                r#"{"main": "lib/entry.js"}"#,
            ),
            ("/app/node_modules/with-main/lib/entry.js", ""),
            (
                "/app/node_modules/with-exports/package.json",
                r#"{"exports": {"import": "./esm.js", "node": "./node.js"}}"#,
            ),
            ("/app/node_modules/with-exports/node.js", ""),
        ]);
        let main = resolve_from(&fs, "with-main", "/app/main.js").unwrap();
        assert_eq!(
            main.path,
            PathBuf::from("/app/node_modules/with-main/lib/entry.js")
        );

        let export = resolve_from(&fs, "with-exports", "/app/main.js").unwrap();
        assert_eq!(
            export.path,
            PathBuf::from("/app/node_modules/with-exports/node.js")
        );
    }

    #[test]
    fn reports_unexported_subpaths() {
        let fs = MemoryFs::new(&[
            ("/app/main.js", ""),
            (
                "/app/node_modules/pkg/package.json",
                r#"{"exports": {".": "./index.js"}}"#,
            ),
            ("/app/node_modules/pkg/index.js", ""),
        ]);
        assert_eq!(
            resolve_from(&fs, "pkg/private", "/app/main.js"),
            Err(Error::NotExported {
                subpath: "private".to_owned()
            })
        );
    }

    #[test]
    fn marks_esm_packages() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "module"}"#),
            ("/app/main.js", ""),
            ("/app/helper.js", ""),
        ]);
        let resolved = resolve_from(&fs, "./helper.js", "/app/main.js").unwrap();
        assert_eq!(resolved.format, Format::Esm);
    }

    #[test]
    fn the_nearest_manifest_decides_the_format() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "module"}"#),
            ("/app/sub/package.json", r#"{"name": "sub"}"#),
            ("/app/sub/helper.js", ""),
        ]);
        assert_eq!(
            format_of(Path::new("/app/sub/helper.js"), &fs),
            Ok(Format::CommonJs)
        );
    }

    #[test]
    fn node_modules_ends_the_package_scope() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "module"}"#),
            ("/app/node_modules/pkg/helper.js", ""),
        ]);
        assert_eq!(
            format_of(Path::new("/app/node_modules/pkg/helper.js"), &fs),
            Ok(Format::CommonJs)
        );
    }

    #[test]
    fn the_filesystem_root_can_be_the_package_scope() {
        let fs = MemoryFs::new(&[
            ("/package.json", r#"{"type": "module"}"#),
            ("/app/helper.js", ""),
        ]);
        assert_eq!(format_of(Path::new("/app/helper.js"), &fs), Ok(Format::Esm));
    }

    #[test]
    fn typescript_modules_are_javascript_but_declarations_are_not() {
        assert!(is_javascript(Path::new("/app/main.ts")));
        assert!(is_javascript(Path::new("/app/main.mts")));
        assert!(is_javascript(Path::new("/app/main.cts")));
        assert!(is_javascript(Path::new("/app/main.tsx")));
        assert!(!is_javascript(Path::new("/app/types.d.ts")));
        assert!(!is_javascript(Path::new("/app/types.d.mts")));
        assert!(!is_javascript(Path::new("/app/types.d.cts")));
    }

    #[test]
    fn typescript_extensions_follow_node_format_rules() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "module"}"#),
            ("/app/main.ts", ""),
            ("/app/main.mts", ""),
            ("/app/main.cts", ""),
            ("/app/legacy/package.json", r#"{"type": "commonjs"}"#),
            ("/app/legacy/main.ts", ""),
        ]);
        assert_eq!(format_of(Path::new("/app/main.ts"), &fs), Ok(Format::Esm));
        assert_eq!(format_of(Path::new("/app/main.mts"), &fs), Ok(Format::Esm));
        assert_eq!(
            format_of(Path::new("/app/main.cts"), &fs),
            Ok(Format::CommonJs)
        );
        assert_eq!(
            format_of(Path::new("/app/legacy/main.ts"), &fs),
            Ok(Format::CommonJs)
        );
    }

    #[test]
    fn node_module_paths_follow_node_search_order() {
        assert_eq!(
            node_module_paths(Path::new("/app/src")),
            vec![
                PathBuf::from("/app/src/node_modules"),
                PathBuf::from("/app/node_modules"),
                PathBuf::from("/node_modules"),
            ]
        );
        assert_eq!(
            node_module_paths(Path::new("/app/node_modules/pkg")),
            vec![
                PathBuf::from("/app/node_modules/pkg/node_modules"),
                PathBuf::from("/app/node_modules"),
                PathBuf::from("/node_modules"),
            ]
        );
    }

    #[test]
    fn node_modules_is_detected_by_path_component() {
        assert!(is_under_node_modules(Path::new(
            "/app/node_modules/pkg/index.ts"
        )));
        assert!(!is_under_node_modules(Path::new(
            "/app/node_modules_cache/index.ts"
        )));
        assert!(!is_under_node_modules(Path::new(
            "/app/mynode_modules/index.ts"
        )));
    }

    #[test]
    fn rejects_native_addons() {
        let fs = MemoryFs::new(&[("/app/main.js", ""), ("/app/addon.node", "")]);
        assert!(matches!(
            resolve_from(&fs, "./addon.node", "/app/main.js"),
            Err(Error::Unsupported { .. })
        ));
    }

    #[test]
    fn errors_render_paths_through_the_caller() {
        let unsupported = Error::Unsupported {
            path: PathBuf::from("/app/addon.node"),
        };
        assert_eq!(
            unsupported.to_string(),
            "/app/addon.node is not a supported module type"
        );
        assert_eq!(
            unsupported.message(|path| path.display().to_string().replace('/', "\\")),
            "\\app\\addon.node is not a supported module type"
        );
    }

    #[test]
    fn missing_modules_are_not_found() {
        let fs = MemoryFs::new(&[("/app/main.js", "")]);
        assert_eq!(
            resolve_from(&fs, "./nope", "/app/main.js"),
            Err(Error::NotFound)
        );
        assert_eq!(
            resolve_from(&fs, "nope", "/app/main.js"),
            Err(Error::NotFound)
        );
    }
}
