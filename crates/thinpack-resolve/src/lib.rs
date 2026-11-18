mod json;

mod package;

use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

use package::Manifest;

pub use package::declares_optional;

pub trait FileSystem {
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;

    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when the path cannot be read.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
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

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("module not found"),
            Self::NotExported { subpath } => {
                write!(formatter, "package subpath '{subpath}' is not exported")
            }
            Self::Unsupported { path } => write!(
                formatter,
                "{} is not a supported module type",
                path.display()
            ),
            Self::InvalidPackage { path, message } => {
                write!(formatter, "invalid {}: {message}", path.display())
            }
            Self::Io { path, message } => write!(formatter, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

/// # Errors
///
/// Returns [`Error::NotFound`] when the specifier cannot be resolved, and the
/// parse, export-map and I/O errors reported by the walked `package.json`
/// manifests.
pub fn resolve(specifier: &str, parent: &Path, fs: &impl FileSystem) -> Result<Resolved, Error> {
    resolve_with(specifier, parent, fs, false)
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
    resolve_with(specifier, parent, fs, true)
}

fn resolve_with(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
    prefer_import: bool,
) -> Result<Resolved, Error> {
    if specifier.starts_with('#') {
        return resolve_package_import(specifier, parent, fs, prefer_import);
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
    load_package(specifier, parent, fs, prefer_import)
}

fn resolve_package_import(
    specifier: &str,
    parent: &Path,
    fs: &impl FileSystem,
    prefer_import: bool,
) -> Result<Resolved, Error> {
    let mut directory = parent.parent().unwrap_or_else(|| Path::new("/"));
    loop {
        let manifest_path = directory.join("package.json");
        if fs.is_file(&manifest_path) {
            let manifest = Manifest::read(&manifest_path, fs)?;
            let Some(imports) = manifest.imports else {
                return Err(Error::NotFound);
            };
            let target = package::resolve_import(&imports, specifier).ok_or(Error::NotFound)?;
            if target.starts_with("./") {
                return load_path(&normalize(&directory.join(target)), fs);
            }
            if target.starts_with("../") {
                return Err(Error::NotFound);
            }
            return resolve_with(&target, &directory.join("index.js"), fs, prefer_import);
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
    prefer_import: bool,
) -> Result<Resolved, Error> {
    let (name, subpath) = split_package(specifier);
    let base = parent.parent().unwrap_or_else(|| Path::new("/"));
    let directory = find_package(name, base, fs).ok_or(Error::NotFound)?;
    let manifest = Manifest::read(&directory.join("package.json"), fs)?;

    if let Some(exports) = &manifest.exports {
        let target =
            package::resolve_subpath(exports, subpath, prefer_import).ok_or_else(|| {
                Error::NotExported {
                    subpath: subpath.to_owned(),
                }
            })?;
        let path = directory.join(target);
        return load_file(&path, fs)?.ok_or(Error::NotFound);
    }

    if subpath.is_empty() {
        if let Some(main) = &manifest.main
            && let Some(resolved) = load_file_or_directory(&directory.join(main), fs)?
        {
            return Ok(resolved);
        }
        return load_index(&directory, fs)?.ok_or(Error::NotFound);
    }

    load_path(&directory.join(subpath), fs)
}

fn find_package(name: &str, base: &Path, fs: &impl FileSystem) -> Option<PathBuf> {
    base.ancestors().find_map(|directory| {
        let candidate = directory.join("node_modules").join(name);
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

fn load_path(path: &Path, fs: &impl FileSystem) -> Result<Resolved, Error> {
    load_file_or_directory(path, fs)?.ok_or(Error::NotFound)
}

fn load_file_or_directory(path: &Path, fs: &impl FileSystem) -> Result<Option<Resolved>, Error> {
    if let Some(resolved) = load_file(path, fs)? {
        return Ok(Some(resolved));
    }
    if fs.is_dir(path) {
        return load_directory(path, fs);
    }
    Ok(None)
}

fn load_file(path: &Path, fs: &impl FileSystem) -> Result<Option<Resolved>, Error> {
    if fs.is_file(path) {
        return resolved(path, fs).map(Some);
    }

    for extension in ["js", "json", "cjs", "mjs"] {
        let candidate = append_extension(path, extension);
        if fs.is_file(&candidate) {
            return resolved(&candidate, fs).map(Some);
        }
    }
    Ok(None)
}

fn load_directory(directory: &Path, fs: &impl FileSystem) -> Result<Option<Resolved>, Error> {
    let manifest = Manifest::read(&directory.join("package.json"), fs)?;
    if let Some(main) = &manifest.main
        && let Some(resolved) = load_file_or_directory(&directory.join(main), fs)?
    {
        return Ok(Some(resolved));
    }
    load_index(directory, fs)
}

fn load_index(directory: &Path, fs: &impl FileSystem) -> Result<Option<Resolved>, Error> {
    for name in ["index.js", "index.json"] {
        let path = directory.join(name);
        if fs.is_file(&path) {
            return resolved(&path, fs).map(Some);
        }
    }
    Ok(None)
}

fn resolved(path: &Path, fs: &impl FileSystem) -> Result<Resolved, Error> {
    Ok(Resolved {
        path: normalize(path),
        format: format_of(path, fs)?,
    })
}

/// # Errors
///
/// Returns [`Error::Unsupported`] for native addons and the manifest errors
/// raised while looking for the nearest `package.json`.
pub fn format_of(path: &Path, fs: &impl FileSystem) -> Result<Format, Error> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => Ok(Format::Json),
        Some("mjs") => Ok(Format::Esm),
        Some("cjs") => Ok(Format::CommonJs),
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
    for ancestor in directory.ancestors() {
        let manifest = Manifest::read(&ancestor.join("package.json"), fs)?;
        if manifest.module_type.is_some() {
            return Ok(manifest.is_module());
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

fn normalize(path: &Path) -> PathBuf {
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
    fn rejects_native_addons() {
        let fs = MemoryFs::new(&[("/app/main.js", ""), ("/app/addon.node", "")]);
        assert!(matches!(
            resolve_from(&fs, "./addon.node", "/app/main.js"),
            Err(Error::Unsupported { .. })
        ));
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
