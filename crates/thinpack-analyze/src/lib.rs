mod require;
mod static_path;
mod typescript;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_ast::ast::{
    Argument, BindingPattern, CallExpression, ExportAllDeclaration, ExportFromDeclaration,
    Expression, IdentifierReference, ImportDeclaration, ImportExpression, NewExpression,
    PropertyKey, StaticMemberExpression, StringLiteral, TryStatement, VariableDeclarator,
};

use oxc_ast_visit::{VisitJs, walk_js};
use oxc_parser::Parser;
use oxc_semantic::{IsGlobalReference, Semantic, SemanticBuilder, SymbolId};
use oxc_span::{SourceType, Span};
use thinpack_resolve::{Conditions, FileSystem, Format};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Warning {
    pub message: String,
    pub strict: bool,
}

/// Which files an implicitly scanned directory contributes.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum StaticDir {
    /// Every file, as `fs.readdir` followed by a load of each name can reach.
    #[default]
    All,
    /// Only files with one of these extensions, for a computed module path whose
    /// literal prefix names the directory and whose suffix names the candidates.
    Extensions(BTreeSet<String>),
}

impl StaticDir {
    fn extensions<'a>(extensions: impl IntoIterator<Item = &'a str>) -> Self {
        Self::Extensions(extensions.into_iter().map(str::to_owned).collect())
    }

    fn modules() -> Self {
        Self::extensions(["js", "cjs", "mjs", "ts", "cts", "mts", "tsx"])
    }

    fn merge(&mut self, other: &Self) {
        match other {
            Self::All => *self = Self::All,
            Self::Extensions(right) => {
                if let Self::Extensions(left) = self {
                    left.extend(right.iter().cloned());
                }
            }
        }
    }

    /// Whether a file in the directory is one of the candidates.
    #[must_use]
    pub fn accepts(&self, path: &Path) -> bool {
        match self {
            Self::All => true,
            Self::Extensions(extensions) => path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(extension)),
        }
    }
}

#[derive(Debug)]
pub struct Analysis {
    pub files: Vec<PathBuf>,
    /// Every file the entry reaches without crossing an optional edge: a guarded
    /// `require`/`import()` (inside `try`) or an `optionalDependencies` entry. Always a
    /// subset of `files`; `merge` preserves that.
    pub required: BTreeSet<PathBuf>,
    /// The `package.json` files the resolver and [`thinpack_resolve::format_of`] read
    /// while analyzing. The loader reads the same ones at run time, so the payload
    /// has to carry them.
    pub manifests: BTreeSet<PathBuf>,
    pub builtins: BTreeMap<String, BTreeSet<PathBuf>>,
    /// Files whose module format packing detected (a typeless `.js` written as an ES
    /// module); the payload records them so the loader reads the same format.
    pub formats: BTreeMap<PathBuf, Format>,
    /// Files the payload carries as data instead of parsing: paths read through
    /// `fs.readFileSync`/`readFile`/`createReadStream`, declaration files a module
    /// names beside itself, and targets named only by
    /// `require.resolve`/`import.meta.resolve`. Resolving a path does not make it a
    /// module, so the analyzer keeps its bytes and parses nothing.
    pub data_files: BTreeSet<PathBuf>,
    /// Directories enumerated through `fs.readdir`/`readdirSync`, or named by a
    /// literal `__dirname` prefix of a computed module load. The CLI expands a
    /// bounded one like an `--include` root, so the files the application can name
    /// are packed and their imports are analyzed. The filter keeps a computed load
    /// to the candidates its literal suffix names.
    pub static_dirs: BTreeMap<PathBuf, StaticDir>,
    /// TypeScript files replaced by the JavaScript packing stripped them to. The
    /// payload stores this text under the original path, so neither the loader nor
    /// the packed app ever evaluates TypeScript.
    pub stripped: BTreeMap<PathBuf, String>,
    pub warnings: Vec<Warning>,
}

impl Analysis {
    pub fn merge(&mut self, other: Self) {
        self.files.extend(other.files);
        self.files.sort();
        self.files.dedup();
        // A file required by either graph is required by the merged graph; the
        // optional files are what remains outside this set.
        self.required.extend(other.required);
        self.manifests.extend(other.manifests);
        for (builtin, files) in other.builtins {
            self.builtins.entry(builtin).or_default().extend(files);
        }
        self.formats.extend(other.formats);
        self.data_files.extend(other.data_files);
        for (directory, filter) in other.static_dirs {
            self.static_dirs
                .entry(directory)
                .and_modify(|existing| existing.merge(&filter))
                .or_insert(filter);
        }
        self.stripped.extend(other.stripped);
        self.warnings.extend(other.warnings);
        self.warnings.sort();
        self.warnings.dedup();
    }
}

#[derive(Debug)]
pub enum Error {
    Read {
        path: PathBuf,
        message: String,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    /// The format the loader will read the file in cannot be decided, or the
    /// source is written in the other format. Either way the file would fail
    /// only once the artifact runs.
    Format {
        path: PathBuf,
        message: String,
    },
    /// The file is TypeScript that packing cannot turn into JavaScript: TypeScript
    /// under `node_modules`, `.tsx`, or syntax Node's strip-only mode rejects
    /// (enums, namespaces, parameter properties, and other constructs that need a
    /// transform).
    Unsupported {
        path: PathBuf,
        message: String,
    },
    Resolve {
        specifier: String,
        from: PathBuf,
        message: String,
    },
    UnknownBuiltin {
        specifier: String,
        from: PathBuf,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, message }
            | Self::Parse { path, message }
            | Self::Format { path, message }
            | Self::Unsupported { path, message } => {
                write!(formatter, "{}: {message}", path.display())
            }
            Self::Resolve {
                specifier,
                from,
                message,
            } => write!(
                formatter,
                "cannot resolve `{specifier}` from {}: {message}",
                from.display()
            ),
            Self::UnknownBuiltin { specifier, from } => write!(
                formatter,
                "unknown builtin `{specifier}` in {}",
                from.display()
            ),
        }
    }
}

impl std::error::Error for Error {}

/// # Errors
///
/// Returns [`Error::Read`] for unreadable or non-UTF-8 modules, [`Error::Parse`]
/// when a module does not parse in the format it will be loaded in, [`Error::Format`]
/// when that format is undecidable or contradicts the source, [`Error::Unsupported`]
/// for TypeScript that cannot be stripped, [`Error::Resolve`] for unresolvable
/// non-optional specifiers and [`Error::UnknownBuiltin`] for `node:` builtins that
/// thinpack does not ship.
pub fn analyze(entry: &Path, fs: &impl FileSystem) -> Result<Analysis, Error> {
    analyze_roots(entry, &[], fs)
}

/// [`analyze`], with package names whose missing dependencies the caller has
/// explicitly excluded (`--allow-missing`).
///
/// # Errors
///
/// Returns the same errors as [`analyze`].
pub fn analyze_with(
    entry: &Path,
    fs: &impl FileSystem,
    allow_missing: &BTreeSet<String>,
    custom_conditions: &[String],
) -> Result<Analysis, Error> {
    analyze_roots_with(entry, &[], fs, allow_missing, custom_conditions)
}

/// Analyze the entry and explicit include roots as one graph.
///
/// Every root is required. A JavaScript root is parsed and its dependencies join
/// the same traversal; any other root is recorded as a required payload file. A
/// module shared by several roots is read and parsed once.
///
/// # Errors
///
/// Returns the same errors as [`analyze`].
pub fn analyze_roots(
    entry: &Path,
    roots: &[PathBuf],
    fs: &impl FileSystem,
) -> Result<Analysis, Error> {
    analyze_roots_with(entry, roots, fs, &BTreeSet::new(), &[])
}

/// [`analyze_roots`], with package names whose missing dependencies the caller has
/// explicitly excluded (`--allow-missing`) and custom package conditions.
///
/// # Errors
///
/// Returns the same errors as [`analyze`].
pub fn analyze_roots_with(
    entry: &Path,
    roots: &[PathBuf],
    fs: &impl FileSystem,
    allow_missing: &BTreeSet<String>,
    custom_conditions: &[String],
) -> Result<Analysis, Error> {
    let fs = ManifestRecorder::new(fs);
    let roots: BTreeSet<PathBuf> = roots.iter().cloned().collect();
    // Declaration files, JSON and assets are payload data, not modules. They join
    // the result as required files after the walk instead of being parsed.
    let (data_roots, module_roots): (Vec<_>, Vec<_>) = roots
        .into_iter()
        .partition(|root| !thinpack_resolve::is_javascript(root));
    // Required targets are queued in front of optional ones, so every root's
    // non-optional closure is complete before any optional file is read and a file
    // reached both ways is classified by its required edge. `files` doubles as the
    // visited set: each module is read and parsed once however many roots reach it.
    let mut pending = VecDeque::with_capacity(module_roots.len() + 1);
    pending.push_back((entry.to_path_buf(), false));
    pending.extend(module_roots.into_iter().map(|root| (root, false)));
    let mut files = BTreeSet::new();
    let mut required = BTreeSet::new();
    let mut builtins: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut formats = BTreeMap::new();
    let mut output = WalkOutput::default();

    while let Some((path, optional_subtree)) = pending.pop_front() {
        if !files.insert(path.clone()) {
            continue;
        }
        if !optional_subtree {
            required.insert(path.clone());
        }
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            continue;
        }

        let optional_dependencies = thinpack_resolve::optional_dependencies(&path, &fs);
        let parsed = load_module(&path, &fs, &mut output)?;
        if let Some(format) = parsed.detected {
            formats.insert(path.clone(), format);
        }
        for ((specifier, is_import), usage) in parsed.specifiers {
            // A dependency listed in `optionalDependencies` is optional even when the
            // request is unguarded, so a hoisted install the payload cannot reach is
            // omitted with a warning instead of failing the pack. The inherited edge is
            // deliberately excluded here: `record_builtin` decides whether to link an
            // optional builtin from the reference itself.
            let direct_optional = usage.optional
                || thinpack_resolve::is_optional_dependency(&specifier, &optional_dependencies);
            if record_builtin(
                &specifier,
                direct_optional,
                &path,
                &mut builtins,
                &mut output.warnings,
            )? {
                continue;
            }
            // Every file under an optional edge is optional, however the file itself
            // reaches its dependencies. Required targets are visited first, so a file
            // also reachable by a required edge still fails the pack.
            let optional_edge = optional_subtree || direct_optional;
            let conditions = Conditions {
                import: is_import,
                custom: custom_conditions.to_vec(),
            };
            let Some(resolved) = resolve_dependency(
                &specifier,
                optional_edge,
                &path,
                &fs,
                &conditions,
                allow_missing,
                &mut output.warnings,
            )?
            else {
                continue;
            };
            // A resolve-only reference only locates the target, so its bytes join
            // the payload without being parsed.
            if usage.resolve_only {
                if resolved
                    .file_name()
                    .is_some_and(|name| name == "package.json")
                    && let Some(bins) = parsed.bin_selections.get(&specifier)
                {
                    include_selected_bins(
                        &resolved,
                        bins,
                        optional_edge,
                        &fs,
                        &mut pending,
                        &mut output.data_files,
                        &mut output.warnings,
                    )?;
                }
                output.data_files.insert(resolved);
            } else if optional_edge {
                pending.push_back((resolved, true));
            } else {
                pending.push_front((resolved, false));
            }
        }
    }

    output.warnings.sort();
    output.warnings.dedup();
    // An explicit include is required even if the graph only reaches it through an
    // optional edge, or does not reach it at all.
    files.extend(data_roots.iter().cloned());
    required.extend(data_roots);
    Ok(Analysis {
        files: files.into_iter().collect(),
        required,
        manifests: fs.into_manifests(),
        builtins,
        formats,
        data_files: output.data_files,
        static_dirs: output.static_dirs,
        stripped: output.stripped,
        warnings: output.warnings,
    })
}

fn include_selected_bins(
    manifest: &Path,
    bins: &BTreeSet<String>,
    optional: bool,
    fs: &impl FileSystem,
    pending: &mut VecDeque<(PathBuf, bool)>,
    data_files: &mut BTreeSet<PathBuf>,
    warnings: &mut Vec<Warning>,
) -> Result<(), Error> {
    for bin in bins {
        include_selected_bin(manifest, bin, optional, fs, pending, data_files, warnings)?;
    }
    Ok(())
}

/// Queue one executable target that a computed module load selected from a
/// resolved package manifest. The manifest can be resolved for its version without
/// selecting anything; only a literal `pkg.bin[name]` used as a module target
/// reaches this path.
fn include_selected_bin(
    manifest: &Path,
    name: &str,
    optional: bool,
    fs: &impl FileSystem,
    pending: &mut VecDeque<(PathBuf, bool)>,
    data_files: &mut BTreeSet<PathBuf>,
    warnings: &mut Vec<Warning>,
) -> Result<(), Error> {
    let bin =
        thinpack_resolve::declared_bin(manifest, name, fs).map_err(|error| Error::Format {
            path: manifest.to_path_buf(),
            message: error.to_string(),
        })?;
    let Some(bin) = bin else {
        return Ok(());
    };
    if !fs.is_file(&bin) {
        warnings.push(Warning {
            message: format!(
                "{}: declared bin {} is missing; the app must handle the error",
                manifest.display(),
                bin.display()
            ),
            strict: false,
        });
        return Ok(());
    }
    if thinpack_resolve::is_javascript(&bin) {
        if optional {
            pending.push_back((bin, true));
        } else {
            pending.push_front((bin, false));
        }
    } else {
        data_files.insert(bin);
    }
    Ok(())
}

/// Resolves one dependency edge. `None` means an optional or `--allow-missing`
/// target is absent: both warn because the app owns the failure. Any other
/// resolution error fails the pack.
fn resolve_dependency(
    specifier: &str,
    optional: bool,
    from: &Path,
    fs: &impl FileSystem,
    conditions: &Conditions,
    allow_missing: &BTreeSet<String>,
    warnings: &mut Vec<Warning>,
) -> Result<Option<PathBuf>, Error> {
    match thinpack_resolve::resolve_path_with_conditions(specifier, from, fs, conditions) {
        Ok(resolved) => Ok(Some(resolved)),
        Err(error) if optional => {
            warnings.push(Warning {
                message: format!(
                    "{}: optional dependency `{specifier}` is missing ({error}); the app must handle the error",
                    from.display()
                ),
                strict: false,
            });
            Ok(None)
        }
        Err(error) if allow_missing.contains(thinpack_resolve::package_name(specifier)) => {
            warnings.push(Warning {
                message: format!(
                    "{}: dependency `{specifier}` is missing ({error}); excluded by --allow-missing",
                    from.display()
                ),
                strict: false,
            });
            Ok(None)
        }
        Err(error) => Err(Error::Resolve {
            specifier: specifier.to_owned(),
            from: from.to_path_buf(),
            message: error.to_string(),
        }),
    }
}

/// A [`FileSystem`] that remembers every `package.json` it reads. The resolver and
/// [`thinpack_resolve::format_of`] only read a manifest when the loader will read it
/// too, so the recorded set is exactly what the payload has to carry.
struct ManifestRecorder<'a, F> {
    inner: &'a F,
    manifests: RefCell<BTreeSet<PathBuf>>,
}

impl<'a, F: FileSystem> ManifestRecorder<'a, F> {
    const fn new(inner: &'a F) -> Self {
        Self {
            inner,
            manifests: RefCell::new(BTreeSet::new()),
        }
    }

    fn into_manifests(self) -> BTreeSet<PathBuf> {
        self.manifests.into_inner()
    }
}

impl<F: FileSystem> FileSystem for ManifestRecorder<'_, F> {
    fn is_file(&self, path: &Path) -> bool {
        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let bytes = self.inner.read(path)?;
        if path.file_name().is_some_and(|name| name == "package.json") {
            self.manifests.borrow_mut().insert(path.to_path_buf());
        }
        Ok(bytes)
    }
}

/// How the module graph uses a specifier.
#[derive(Clone, Copy, Debug)]
struct Usage {
    /// Every reference is guarded (a `try` block or `process.getBuiltinModule`), so a
    /// failed lookup is the app's to handle.
    optional: bool,
    /// Every reference only locates the target, so it joins the payload as data
    /// instead of being parsed as a module.
    resolve_only: bool,
}

type Specifiers = BTreeMap<(String, bool), Usage>;

/// Records `specifier` when it names a builtin and reports whether it did. A
/// guarded reference to a builtin that may be absent only warns; an unguarded
/// unknown `node:` builtin fails the pack.
fn record_builtin(
    specifier: &str,
    optional: bool,
    from: &Path,
    builtins: &mut BTreeMap<String, BTreeSet<PathBuf>>,
    warnings: &mut Vec<Warning>,
) -> Result<bool, Error> {
    let Some(module) = thinpack_caps::module_for_builtin(specifier) else {
        if !specifier.starts_with("node:") {
            return Ok(false);
        }
        // A guarded `require` or a `process.getBuiltinModule` probe is how an app
        // checks for a builtin that may not ship; warn like an optional dependency
        // instead of failing the pack.
        if optional {
            warnings.push(Warning {
                message: format!(
                    "{}: unknown builtin `{specifier}` may be absent; the app must handle ERR_UNKNOWN_BUILTIN_MODULE",
                    from.display()
                ),
                strict: false,
            });
            return Ok(true);
        }
        return Err(Error::UnknownBuiltin {
            specifier: specifier.to_owned(),
            from: from.to_path_buf(),
        });
    };
    if module.optional && optional {
        warnings.push(Warning {
            message: format!(
                "{}: optional builtin `{specifier}` may be absent; the app must handle ERR_UNKNOWN_BUILTIN_MODULE",
                from.display()
            ),
            strict: false,
        });
        return Ok(true);
    }
    let builtin = specifier
        .strip_prefix("node:")
        .unwrap_or(specifier)
        .to_owned();
    builtins
        .entry(builtin)
        .or_default()
        .insert(from.to_path_buf());
    Ok(true)
}

/// The parser dialect for a file: the extension supplies the language and
/// [`thinpack_resolve::format_of`] supplies the module kind the loader will read it
/// in. Parsing with the loader's rule — instead of letting a generic `.js` accept
/// both dialects — is what makes packing reject a file whose syntax and format
/// disagree, rather than leaving that failure to the artifact.
///
/// TypeScript reaches this point after [`typescript::strip`], so it parses as
/// JavaScript. `.mts`/`.cts` fix their format like `.mjs`/`.cjs`, and their format
/// is therefore not diagnosed against the manifest; `.ts` follows the `.js` rules.
fn parser_dialect(
    path: &Path,
    fs: &impl FileSystem,
) -> Result<(SourceType, Option<Format>), Error> {
    let extension = path.extension().and_then(|extension| extension.to_str());
    let source_type = match extension {
        Some("ts") => SourceType::unambiguous(),
        Some("mts") => SourceType::mjs(),
        Some("cts") => SourceType::cjs(),
        _ => SourceType::from_path(path).unwrap_or_default(),
    };
    let format_from_extension = matches!(extension, Some("cjs" | "mjs" | "cts" | "mts"));
    if format_from_extension {
        return Ok((source_type, None));
    }
    let format = thinpack_resolve::format_of(path, fs).map_err(|error| Error::Format {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let source_type = match format {
        Format::Esm => source_type.with_module(true),
        Format::CommonJs => source_type.with_commonjs(true),
        // JSON is data, not a module; `analyze` skips it before parsing.
        Format::Json => source_type,
    };
    Ok((source_type, Some(format)))
}

/// Explains a file whose syntax belongs to the other module format than the one its
/// path resolves to. The loader would reject it after the artifact is written, so
/// packing fails first and names the two ways to ask for the format it is written in.
/// Packing reads the file on disk; a hook that assigns a format or source only runs
/// in the artifact, so the message says why a hook cannot resolve this failure.
fn format_mismatch(
    source: &str,
    path: &Path,
    allocator: &Allocator,
    source_type: SourceType,
    format: Format,
) -> Option<String> {
    // A TypeScript file switches formats through its TypeScript extension, the
    // same choice Node documents for `.ts`.
    let commonjs = if thinpack_resolve::is_typescript(path) {
        ".cts"
    } else {
        ".cjs"
    };
    let esm = if thinpack_resolve::is_typescript(path) {
        ".mts"
    } else {
        ".mjs"
    };
    let (other, message) = match format {
        Format::Esm => (
            source_type.with_commonjs(true),
            format!(
                "uses CommonJS syntax but is loaded as an ES module; rename it to {commonjs} or remove \"type\": \"module\" from the nearest package.json. The analyzer does not run loader hooks; it uses the source and format on disk"
            ),
        ),
        Format::CommonJs => (
            source_type.with_module(true),
            format!(
                "uses ES module syntax but is loaded as CommonJS; rename it to {esm} or add \"type\": \"module\" to the nearest package.json. The analyzer does not run loader hooks; it uses the source and format on disk"
            ),
        ),
        Format::Json => return None,
    };
    Parser::new(allocator, source, other)
        .parse()
        .diagnostics
        .is_empty()
        .then_some(message)
}

/// What the walk collects besides the module edges themselves.
#[derive(Default)]
struct WalkOutput {
    warnings: Vec<Warning>,
    data_files: BTreeSet<PathBuf>,
    static_dirs: BTreeMap<PathBuf, StaticDir>,
    stripped: BTreeMap<PathBuf, String>,
}

struct ParsedModule {
    specifiers: Specifiers,
    detected: Option<Format>,
    // Manifest specifiers resolve relative to this module, not the whole graph.
    bin_selections: BTreeMap<String, BTreeSet<String>>,
}

fn load_module(
    path: &Path,
    fs: &impl FileSystem,
    output: &mut WalkOutput,
) -> Result<ParsedModule, Error> {
    let bytes = fs.read(path).map_err(|error| Error::Read {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let source = String::from_utf8(bytes).map_err(|_| Error::Read {
        path: path.to_path_buf(),
        message: "module is not valid UTF-8".to_owned(),
    })?;
    let source = typescript::strip(path, source, &mut output.stripped)?;
    parse_specifiers(&source, path, fs, output)
}

fn parse_specifiers(
    source: &str,
    path: &Path,
    fs: &impl FileSystem,
    output: &mut WalkOutput,
) -> Result<ParsedModule, Error> {
    let allocator = Allocator::default();
    let (source_type, format) = parser_dialect(path, fs)?;
    let mut parsed = Parser::new(&allocator, source, source_type).parse();
    let mut detected = None;
    if !parsed.diagnostics.is_empty() && format == Some(Format::CommonJs) {
        // Node's detect-module: a typeless `.js` that does not parse as CommonJS is
        // reparsed as an ES module. An explicit `"type": "commonjs"` keeps the strict
        // rule, so only a missing type opts in.
        let package_type =
            thinpack_resolve::package_type(path, fs).map_err(|error| Error::Format {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        if package_type.is_none() {
            let esm_type = source_type.with_module(true);
            let esm = Parser::new(&allocator, source, esm_type).parse();
            if esm.diagnostics.is_empty() {
                output.warnings.push(Warning {
                    message: format!(
                        "{}: contains ES module syntax in a package without \"type\"; packed as an ES module. Add \"type\": \"module\" to the nearest package.json to make the format explicit",
                        path.display()
                    ),
                    strict: false,
                });
                parsed = esm;
                detected = Some(Format::Esm);
            }
        }
    }
    if !parsed.diagnostics.is_empty() {
        if let Some(format) = format
            && let Some(message) = format_mismatch(source, path, &allocator, source_type, format)
        {
            return Err(Error::Format {
                path: path.to_path_buf(),
                message,
            });
        }
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(Error::Parse {
            path: path.to_path_buf(),
            message: messages.join("; "),
        });
    }

    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program)
        .semantic;
    let late_import_bindings = require::late_import_bindings(&semantic);
    let late_import_factories = late_import_bindings
        .values()
        .map(|late_import| late_import.span)
        .collect();
    let mut collector = SpecifierCollector {
        path,
        specifiers: BTreeMap::new(),
        try_depth: 0,
        semantic: &semantic,
        require_bindings: require::bindings(&semantic),
        late_import_bindings,
        late_import_factories,
        manifest_bindings: BTreeMap::new(),
        warnings: &mut output.warnings,
        data_files: &mut output.data_files,
        static_dirs: &mut output.static_dirs,
        bin_selections: BTreeMap::new(),
        fs,
    };
    collector.visit_program(&parsed.program);
    Ok(ParsedModule {
        specifiers: collector.specifiers,
        detected,
        bin_selections: collector.bin_selections,
    })
}

/// The path expression a module load names, unwrapping `pathToFileURL(...)`.
fn module_target<'a>(expression: &'a Expression<'a>) -> &'a Expression<'a> {
    let Expression::CallExpression(call) = expression.without_parentheses() else {
        return expression;
    };
    let is_path_to_file_url = matches!(
        call.callee.without_parentheses(),
        Expression::Identifier(identifier) if identifier.name == "pathToFileURL"
    ) || matches!(
        call.callee.without_parentheses(),
        Expression::StaticMemberExpression(member) if member.property.name == "pathToFileURL"
    );
    if is_path_to_file_url
        && let Some(argument) = call.arguments.first().and_then(Argument::as_expression)
    {
        return argument.without_parentheses();
    }
    expression
}

fn js_string(value: oxc_str::JSStr<'_>) -> String {
    value
        .as_str()
        .map_or_else(|| value.display().to_string(), str::to_owned)
}

/// The global an expression names, either bare or through `globalThis`.
fn global_name<'a>(expression: &'a Expression<'_>) -> Option<&'a str> {
    match expression {
        Expression::Identifier(identifier) => Some(identifier.name.as_str()),
        Expression::StaticMemberExpression(member) => match &member.object {
            Expression::Identifier(object) if object.name == "globalThis" => {
                Some(member.property.name.as_str())
            }
            _ => None,
        },
        _ => None,
    }
}

struct SpecifierCollector<'a, 's> {
    path: &'s Path,
    specifiers: Specifiers,
    try_depth: usize,
    semantic: &'s Semantic<'a>,
    require_bindings: BTreeSet<SymbolId>,
    late_import_bindings: BTreeMap<SymbolId, require::LateImport>,
    /// The constructor expressions that belong to a stable late-import binding.
    late_import_factories: BTreeSet<Span>,
    /// Bindings assigned by `require.resolve`/`import.meta.resolve` to a literal
    /// package manifest.
    manifest_bindings: BTreeMap<SymbolId, BTreeSet<String>>,
    warnings: &'s mut Vec<Warning>,
    data_files: &'s mut BTreeSet<PathBuf>,
    static_dirs: &'s mut BTreeMap<PathBuf, StaticDir>,
    bin_selections: BTreeMap<String, BTreeSet<String>>,
    fs: &'s dyn FileSystem,
}

impl SpecifierCollector<'_, '_> {
    fn add_specifier(&mut self, specifier: String, is_import: bool) {
        self.add(specifier, is_import, self.try_depth > 0, false);
    }

    fn add(&mut self, specifier: String, is_import: bool, optional: bool, resolve_only: bool) {
        self.specifiers
            .entry((specifier, is_import))
            .and_modify(|existing| {
                // A load in any reference makes the file a module; an unguarded
                // reference makes the edge required.
                existing.optional &= optional;
                existing.resolve_only &= resolve_only;
            })
            .or_insert(Usage {
                optional,
                resolve_only,
            });
    }

    fn add_builtins(&mut self, builtins: &[&str]) {
        for builtin in builtins {
            self.add_specifier((*builtin).to_owned(), false);
        }
    }

    /// A static reference to a global selects the capability that owns it, so the
    /// whole shared closure follows the reference instead of a local call name.
    fn add_global(&mut self, name: &str) {
        if let Some(module) = thinpack_caps::module_for_global(name) {
            self.add_builtins(module.builtins);
        }
    }

    fn add_global_reference(&mut self, identifier: &IdentifierReference<'_>) {
        if identifier.is_global_reference(self.semantic.scoping()) {
            self.add_global(identifier.name.as_str());
        }
    }

    fn is_require_binding(&self, identifier: &IdentifierReference<'_>) -> bool {
        // A parameter or local `require` shadows the runtime one; only the ambient
        // binding (or a createRequire result) is a dependency edge.
        (identifier.name == "require" && identifier.is_global_reference(self.semantic.scoping()))
            || require::referenced_symbol(identifier, self.semantic.scoping())
                .is_some_and(|symbol| self.require_bindings.contains(&symbol))
    }

    /// A call through a stable `Function`-built import factory names its target the
    /// same way `import(...)` does.
    fn late_import_call(&mut self, call: &CallExpression<'_>) {
        let Expression::Identifier(identifier) = call.callee.without_parentheses() else {
            return;
        };
        let Some(symbol) = require::referenced_symbol(identifier, self.semantic.scoping()) else {
            return;
        };
        let Some(argument_index) = self
            .late_import_bindings
            .get(&symbol)
            .map(|late_import| late_import.argument)
        else {
            return;
        };
        let complete = call
            .arguments
            .get(argument_index)
            .and_then(Argument::as_expression)
            .is_some_and(|argument| {
                require::specifiers(argument, self.semantic, |specifier| {
                    self.add(specifier, true, self.try_depth > 0, false);
                })
            });
        if !complete {
            if let Some(argument) = call
                .arguments
                .get(argument_index)
                .and_then(Argument::as_expression)
            {
                self.record_bin_selection(argument);
            }
            self.warnings.push(Warning {
                message: format!(
                    "{}: a `Function`-built import with a non-literal argument cannot be analyzed; add computed local files with `--include <path>` (relative to the project root)",
                    self.path.display()
                ),
                strict: true,
            });
        }
    }

    fn require_call(&mut self, call: &CallExpression<'_>) {
        // (receiver or callee, member suffix, resolves as import, never throws in
        // Node, only locates the target)
        let target = match call.callee.without_parentheses() {
            Expression::Identifier(identifier) if self.is_require_binding(identifier) => {
                Some((identifier.name.as_str(), "", false, false, false))
            }
            Expression::StaticMemberExpression(member) => match &member.object {
                Expression::ImportMeta(_) if member.property.name == "resolve" => {
                    Some(("import.meta", ".resolve", true, false, true))
                }
                Expression::Identifier(object) => {
                    match (object.name.as_str(), member.property.name.as_str()) {
                        (name, "resolve") if self.is_require_binding(object) => {
                            Some((name, ".resolve", false, false, true))
                        }
                        ("module", "require") => Some(("module", ".require", false, false, false)),
                        // `getBuiltinModule` returns `null` for a missing module
                        // instead of throwing; a failed lookup is a warning,
                        // not a pack error.
                        ("process", "getBuiltinModule") => {
                            Some(("process", ".getBuiltinModule", false, true, false))
                        }
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => None,
        };
        let Some((target, member, is_import, optional, resolve_only)) = target else {
            return;
        };

        let Some(argument) = call.arguments.first() else {
            return;
        };
        let expression = argument.as_expression();
        let optional = optional || self.try_depth > 0;
        let mut resolved = Vec::new();
        let complete = expression.is_some_and(|argument| {
            if resolve_only
                && let Some(specifier) = require::static_string(argument, self.semantic)
                && specifier.ends_with("/package.json")
            {
                resolved.push(specifier.clone());
                self.add(specifier, is_import, optional, true);
                return true;
            }
            require::specifiers(argument, self.semantic, |specifier| {
                if resolve_only {
                    resolved.push(specifier.clone());
                }
                self.add(specifier, is_import, optional, resolve_only);
            }) || self.add_constant_module_path(argument, is_import, optional, resolve_only)
        });
        if complete && resolve_only {
            self.record_manifest_binding(call, &resolved);
        }
        let static_module_dir = !optional
            && !resolve_only
            && expression.is_some_and(|argument| self.add_static_module_dir(argument));
        if !complete && !static_module_dir {
            if let Some(argument) = expression.filter(|_| !resolve_only) {
                self.record_bin_selection(argument);
            }
            self.warnings.push(Warning {
                message: format!(
                    "{}: `{target}{member}(...)` with a non-literal argument cannot be analyzed. Add required built-ins with `--with <module>`, and computed local files with `--include <path>` (relative to the project root).",
                    self.path.display()
                ),
                strict: true,
            });
        }
    }

    fn record_manifest_binding(&mut self, call: &CallExpression<'_>, specifiers: &[String]) {
        if specifiers.is_empty() {
            return;
        }
        let AstKind::VariableDeclarator(declaration) =
            self.semantic.nodes().parent_kind(call.node_id.get())
        else {
            return;
        };
        let Some(symbol) = declaration
            .id
            .get_binding_identifier()
            .and_then(|identifier| identifier.symbol_id.get())
        else {
            return;
        };
        self.manifest_bindings
            .entry(symbol)
            .or_default()
            .extend(specifiers.iter().cloned());
    }

    fn record_bin_selection(&mut self, expression: &Expression<'_>) {
        let selected = require::selected_bins(expression, self.semantic, &self.manifest_bindings);
        for (manifest, bin) in selected {
            self.bin_selections.entry(manifest).or_default().insert(bin);
        }
    }

    /// `new Function(...)`/`Function(...)` hides imports from the analyzer. Warn only
    /// when a literal body actually mentions `require`/`import`, so ordinary dynamic
    /// expressions stay quiet. A body that is only `return import(<parameter>)` is a
    /// late-import factory instead; its stable binding and direct calls are analyzed
    /// separately, and any other form keeps a strict warning.
    fn dynamic_function(
        &mut self,
        span: Span,
        callee: &Expression<'_>,
        arguments: &[Argument<'_>],
    ) {
        if require::late_import(callee, arguments, self.semantic).is_some() {
            if !self.late_import_factories.contains(&span) {
                self.warnings.push(Warning {
                    message: format!(
                        "{}: a `Function`-built import is not confined to direct calls in this module; add the files it loads with `--include <path>` (relative to the project root)",
                        self.path.display()
                    ),
                    strict: true,
                });
            }
            return;
        }
        if global_name(callee).is_none_or(|name| name != "Function") {
            return;
        }
        let body = arguments
            .iter()
            .filter_map(Argument::as_expression)
            .filter_map(|expression| match expression {
                Expression::StringLiteral(literal) => Some(js_string(literal.value)),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !body.contains("require(") && !body.contains("import(") {
            return;
        }
        self.warnings.push(Warning {
            message: format!(
                "{}: code built with `Function` is not analyzed; add the files it loads with `--include <path>` (relative to the project root)",
                self.path.display()
            ),
            strict: false,
        });
    }
    /// Record a package-relative asset read through `fs.readFileSync`, `readFile` or
    /// `createReadStream` when the path is built from `__dirname`. This is the
    /// `errorhandler` case: a runtime asset that no import reaches.
    fn static_asset(&mut self, call: &CallExpression<'_>) {
        let Some(member) = call.callee.as_member_expression() else {
            return;
        };
        let Some(property) = member
            .static_property_name()
            .and_then(oxc_str::JSStr::as_str)
        else {
            return;
        };
        if !matches!(property, "readFile" | "readFileSync" | "createReadStream") {
            return;
        }
        let Some(path) = self.static_path_argument(call) else {
            return;
        };
        if self.fs.is_file(&path) {
            self.data_files.insert(path);
        } else {
            self.warnings.push(Warning {
                message: format!(
                    "{}: static asset {} is missing; add it with --include or --asset",
                    self.path.display(),
                    path.display()
                ),
                strict: false,
            });
        }
    }

    /// Record a require of one constant path expression rooted away from `cwd`,
    /// such as npm's `require(path.resolve(__dirname, 'cli/entry.js'))`. A path
    /// that depends on `cwd` is left to the dynamic warning.
    fn add_constant_module_path(
        &mut self,
        expression: &Expression<'_>,
        is_import: bool,
        optional: bool,
        resolve_only: bool,
    ) -> bool {
        let base = self.path.parent().unwrap_or_else(|| Path::new("/"));
        let Some(path) =
            static_path::evaluate_rooted(module_target(expression), base, self.semantic)
        else {
            return false;
        };
        // A drive-less virtual root (`/app`) renders with backslashes on Windows;
        // keep the leading `/` the resolver's absolute-specifier check expects.
        let specifier = if path.has_root() && !path.is_absolute() {
            path.to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        } else {
            path.to_string_lossy().into_owned()
        };
        self.add(specifier, is_import, optional, resolve_only);
        true
    }

    /// Record the directory named by a computed module load when the loaded
    /// binding has a `path.join`/`path.resolve` assignment with a literal
    /// `__dirname` prefix and one final computed segment. The filter keeps only
    /// files with the literal suffix that segment appends, or JavaScript modules
    /// when it has none.
    fn add_static_module_dir(&mut self, expression: &Expression<'_>) -> bool {
        let expression = module_target(expression);
        let values: Vec<&Expression<'_>> = match expression.without_parentheses() {
            Expression::Identifier(identifier) => {
                let Some(symbol) = require::referenced_symbol(identifier, self.semantic.scoping())
                else {
                    return false;
                };
                require::assigned_values(symbol, self.semantic)
            }
            _ => vec![expression],
        };
        let base = self.path.parent().unwrap_or_else(|| Path::new("/"));
        let mut found = false;
        for value in values {
            let Some((directory, extension)) =
                static_path::module_directory(value, base, self.semantic)
            else {
                continue;
            };
            if !self.fs.is_dir(&directory) {
                continue;
            }
            let filter = extension.map_or_else(StaticDir::modules, |extension| {
                StaticDir::extensions([extension])
            });
            self.static_dirs
                .entry(directory)
                .and_modify(|existing| existing.merge(&filter))
                .or_insert(filter);
            found = true;
        }
        found
    }

    /// Record a package-relative directory enumerated through `fs.readdir` or
    /// `fs.readdirSync`. The names it returns are not known statically, but the
    /// directory is, so it can be packed like `--include <directory>`. Only a call
    /// on the `fs` module counts: another object's `readdirSync` does not enumerate
    /// files the payload carries.
    fn static_directory(&mut self, call: &CallExpression<'_>) {
        let Some(member) = call.callee.as_member_expression() else {
            return;
        };
        let Expression::Identifier(receiver) = member.object() else {
            return;
        };
        if !self.is_fs_binding(receiver)
            || !matches!(
                member
                    .static_property_name()
                    .and_then(oxc_str::JSStr::as_str),
                Some("readdir" | "readdirSync")
            )
        {
            return;
        }
        let Some(path) = self.static_path_argument(call) else {
            return;
        };
        if self.fs.is_dir(&path) {
            self.static_dirs
                .entry(path)
                .and_modify(|filter| filter.merge(&StaticDir::All))
                .or_insert(StaticDir::All);
        }
    }

    /// Whether `identifier` names the `fs` module, bound once to `require('fs')`
    /// or `require('node:fs')`.
    fn is_fs_binding(&self, identifier: &IdentifierReference<'_>) -> bool {
        let Some(symbol) = require::referenced_symbol(identifier, self.semantic.scoping()) else {
            return false;
        };
        let Some(Expression::CallExpression(call)) = require::initializer(symbol, self.semantic)
        else {
            return false;
        };
        let Some(callee) = call.callee.without_parentheses().get_identifier_reference() else {
            return false;
        };
        self.is_require_binding(callee)
            && matches!(
                call.arguments.first().and_then(Argument::as_expression),
                Some(Expression::StringLiteral(literal))
                    if matches!(literal.value.as_str(), Some("fs" | "node:fs"))
            )
    }

    fn static_path_argument(&self, call: &CallExpression<'_>) -> Option<PathBuf> {
        let expression = call.arguments.first().and_then(Argument::as_expression)?;
        static_path::evaluate(
            expression,
            self.path.parent().unwrap_or_else(|| Path::new("/")),
            self.semantic,
        )
    }

    /// Keep a declaration file the module names and that sits beside it. Programs
    /// such as the TypeScript compiler build the path from `__filename` and pass
    /// the name through tables, so no call argument is statically evaluable; the
    /// name itself is still a literal. Declarations are data, so the app can read
    /// them without the analyzer parsing anything.
    fn sibling_declaration(&mut self, literal: &StringLiteral<'_>) {
        let Some(name) = literal.value.as_str() else {
            return;
        };
        let name = Path::new(name);
        if !thinpack_resolve::is_declaration(name)
            || name.file_name().and_then(|file_name| file_name.to_str()) != name.to_str()
        {
            return;
        }
        let path = self
            .path
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .join(name);
        if self.fs.is_file(&path) {
            self.data_files.insert(path);
        }
    }
}

impl<'a> VisitJs<'a> for SpecifierCollector<'a, '_> {
    fn visit_import_declaration(&mut self, declaration: &ImportDeclaration<'a>) {
        self.add_specifier(js_string(declaration.source.value), true);
        walk_js::walk_import_declaration(self, declaration);
    }

    fn visit_export_from_declaration(&mut self, declaration: &ExportFromDeclaration<'a>) {
        self.add_specifier(js_string(declaration.source.value), true);
        walk_js::walk_export_from_declaration(self, declaration);
    }

    fn visit_export_all_declaration(&mut self, declaration: &ExportAllDeclaration<'a>) {
        self.add_specifier(js_string(declaration.source.value), true);
        walk_js::walk_export_all_declaration(self, declaration);
    }

    fn visit_import_expression(&mut self, expression: &ImportExpression<'a>) {
        if let Expression::StringLiteral(literal) = &expression.source {
            self.add_specifier(js_string(literal.value), true);
        } else {
            self.record_bin_selection(&expression.source);
            if !self.add_static_module_dir(&expression.source) {
                self.warnings.push(Warning {
                    message: format!(
                        "{}: `import(...)` with a non-literal argument cannot be analyzed; add computed local files with `--include <path>` (relative to the project root)",
                        self.path.display()
                    ),
                    strict: true,
                });
            }
        }
        walk_js::walk_import_expression(self, expression);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        self.dynamic_function(call.span, &call.callee, &call.arguments);
        self.static_asset(call);
        self.static_directory(call);
        self.late_import_call(call);
        self.require_call(call);
        walk_js::walk_call_expression(self, call);
    }

    fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
        self.add_global_reference(identifier);
        walk_js::walk_identifier_reference(self, identifier);
    }

    fn visit_string_literal(&mut self, literal: &StringLiteral<'a>) {
        self.sibling_declaration(literal);
        walk_js::walk_string_literal(self, literal);
    }

    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(object) = &member.object
            && object.name == "globalThis"
            && object.is_global_reference(self.semantic.scoping())
        {
            self.add_global(member.property.name.as_str());
        }
        walk_js::walk_static_member_expression(self, member);
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let Some(Expression::Identifier(init)) = declarator.init.as_ref()
            && init.name == "globalThis"
            && init.is_global_reference(self.semantic.scoping())
            && let BindingPattern::ObjectPattern(pattern) = &declarator.id
        {
            for property in &pattern.properties {
                if let PropertyKey::StaticIdentifier(key) = &property.key {
                    self.add_global(key.name.as_str());
                }
            }
        }
        walk_js::walk_variable_declarator(self, declarator);
    }

    fn visit_try_statement(&mut self, statement: &TryStatement<'a>) {
        // Only the `try` block is guarded: a throw from the handler or the finalizer
        // is not caught by this statement.
        self.try_depth += 1;
        self.visit_block_statement(&statement.block);
        self.try_depth -= 1;
        if let Some(handler) = &statement.handler {
            self.visit_catch_clause(handler);
        }
        if let Some(finalizer) = &statement.finalizer {
            self.visit_block_statement(finalizer);
        }
    }

    fn visit_new_expression(&mut self, expression: &NewExpression<'a>) {
        self.dynamic_function(expression.span, &expression.callee, &expression.arguments);
        walk_js::walk_new_expression(self, expression);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::io;
    use std::path::{Path, PathBuf};

    use super::*;

    struct MemoryFs {
        files: BTreeMap<PathBuf, String>,
        directories: BTreeSet<PathBuf>,
        reads: RefCell<BTreeMap<PathBuf, usize>>,
    }

    impl MemoryFs {
        fn new(files: &[(&str, &str)]) -> Self {
            let mut fs = Self {
                files: BTreeMap::new(),
                directories: BTreeSet::new(),
                reads: RefCell::new(BTreeMap::new()),
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
            let contents = self
                .files
                .get(path)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            *self
                .reads
                .borrow_mut()
                .entry(path.to_path_buf())
                .or_default() += 1;
            Ok(contents.clone().into_bytes())
        }
    }

    fn capabilities(analysis: &Analysis) -> Vec<String> {
        let roots: Vec<String> = analysis
            .builtins
            .keys()
            .filter_map(|builtin| thinpack_caps::module_for_builtin(builtin))
            .map(|module| module.name.to_owned())
            .collect();
        thinpack_caps::closure(&roots)
            .unwrap()
            .iter()
            .map(|module| module.name.to_owned())
            .collect()
    }

    #[test]
    fn walks_relative_and_builtin_requires() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", "{}"),
            (
                "/app/main.js",
                "const path = require('node:path');\nrequire('./lib');\n",
            ),
            ("/app/lib/index.js", "require('node:events');\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert_eq!(
            analysis.files,
            vec![
                PathBuf::from("/app/lib/index.js"),
                PathBuf::from("/app/main.js")
            ]
        );
        assert_eq!(capabilities(&analysis), vec!["events", "path", "process"]);
        assert_eq!(
            analysis.builtins["events"],
            BTreeSet::from([PathBuf::from("/app/lib/index.js")])
        );
        assert_eq!(analysis.warnings, Vec::new());
    }

    #[test]
    fn collects_esm_specifiers() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "module"}"#),
            (
                "/app/main.js",
                "import './a.js';\nexport * from './b.js';\n",
            ),
            ("/app/a.js", "export const a = 1;\n"),
            ("/app/b.js", "export const b = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert_eq!(analysis.files.len(), 3);
    }

    #[test]
    fn a_nested_manifest_decides_the_dialect_of_its_files() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"name": "app"}"#),
            ("/app/main.js", "require('./sub/x.js');\n"),
            ("/app/sub/package.json", r#"{"type": "module"}"#),
            ("/app/sub/x.js", "export const x = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(analysis.files.contains(&PathBuf::from("/app/sub/x.js")));
    }

    #[test]
    fn typescript_is_stripped_and_format_checked() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "module"}"#),
            (
                "/app/main.ts",
                "import { helper } from './helper.ts';\nconst value: number = helper(1);\nconsole.log(value);\n",
            ),
            (
                "/app/helper.ts",
                "export function helper(value: number): number { return value; }\n",
            ),
        ]);
        let analysis = analyze(Path::new("/app/main.ts"), &fs).unwrap();
        assert!(analysis.files.contains(&PathBuf::from("/app/helper.ts")));
        assert!(
            analysis
                .manifests
                .contains(&PathBuf::from("/app/package.json"))
        );
        assert!(analysis.formats.is_empty());

        let main = &analysis.stripped[Path::new("/app/main.ts")];
        assert!(!main.contains(": number"), "{main}");
        assert!(main.contains("helper(1)"), "{main}");
        let helper = &analysis.stripped[Path::new("/app/helper.ts")];
        assert!(!helper.contains(": number"), "{helper}");
        assert!(helper.contains("return value;"), "{helper}");
    }

    #[test]
    fn typescript_in_node_modules_is_rejected() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.ts",
                "import { value } from 'dep';\nconsole.log(value);\n",
            ),
            (
                "/app/node_modules/dep/package.json",
                r#"{"name": "dep", "type": "module", "exports": "./index.ts"}"#,
            ),
            (
                "/app/node_modules/dep/index.ts",
                "export const value: number = 1;\n",
            ),
        ]);
        match analyze(Path::new("/app/main.ts"), &fs) {
            Err(Error::Unsupported { path, message }) => {
                assert_eq!(path, PathBuf::from("/app/node_modules/dep/index.ts"));
                assert!(
                    message.contains("ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING"),
                    "{message}"
                );
            }
            other => panic!("expected an unsupported TypeScript error, got {other:?}"),
        }
    }

    #[test]
    fn typescript_type_only_imports_are_erased_before_analysis() {
        let fs = MemoryFs::new(&[(
            "/app/main.ts",
            "import type { Missing } from './missing.ts';\nconst value: Missing = 1 as Missing;\nconsole.log(value);\n",
        )]);
        let analysis = analyze(Path::new("/app/main.ts"), &fs).unwrap();
        assert_eq!(analysis.files, vec![PathBuf::from("/app/main.ts")]);
        assert!(analysis.formats.is_empty());
        assert_eq!(analysis.warnings, Vec::new());
    }

    #[test]
    fn typescript_transform_only_syntax_is_rejected() {
        let fs = MemoryFs::new(&[("/app/main.ts", "enum Mode { A }\nconsole.log(Mode.A);\n")]);
        match analyze(Path::new("/app/main.ts"), &fs) {
            Err(Error::Unsupported { path, message }) => {
                assert_eq!(path, PathBuf::from("/app/main.ts"));
                assert!(
                    message.contains("TypeScript enum is not supported in strip-only mode"),
                    "{message}"
                );
                assert!(message.starts_with("1:"), "{message}");
            }
            other => panic!("expected an unsupported TypeScript error, got {other:?}"),
        }
    }

    #[test]
    fn tsx_is_rejected() {
        let fs = MemoryFs::new(&[("/app/main.tsx", "console.log(<div />);\n")]);
        match analyze(Path::new("/app/main.tsx"), &fs) {
            Err(Error::Unsupported { path, message }) => {
                assert_eq!(path, PathBuf::from("/app/main.tsx"));
                assert!(
                    message.contains("TypeScript JSX (.tsx) is not supported"),
                    "{message}"
                );
            }
            other => panic!("expected an unsupported TypeScript error, got {other:?}"),
        }
    }

    #[test]
    fn mts_and_cts_fix_their_format() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "commonjs"}"#),
            ("/app/main.mts", "export const value: number = 1;\n"),
            (
                "/app/main.cts",
                "const value: number = require('./other.cjs');\nmodule.exports = value;\n",
            ),
            ("/app/other.cjs", "module.exports = 1;\n"),
        ]);
        let esm = analyze(Path::new("/app/main.mts"), &fs).unwrap();
        assert!(esm.formats.is_empty());
        let commonjs = analyze(Path::new("/app/main.cts"), &fs).unwrap();
        assert!(commonjs.formats.is_empty());
        assert!(commonjs.stripped.contains_key(Path::new("/app/main.cts")));
    }

    #[test]
    fn a_ts_file_loaded_as_commonjs_suggests_mts() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "commonjs"}"#),
            ("/app/main.ts", "import 'node:fs';\n"),
        ]);
        match analyze(Path::new("/app/main.ts"), &fs) {
            Err(Error::Format { message, .. }) => {
                assert!(message.contains("rename it to .mts"), "{message}");
                assert!(!message.contains(".mjs"), "{message}");
            }
            other => panic!("expected a format error, got {other:?}"),
        }
    }

    #[test]
    fn a_js_file_loaded_as_commonjs_cannot_use_es_module_syntax() {
        let fs = MemoryFs::new(&[
            (
                "/app/package.json",
                r#"{"name": "esm-bundle", "type": "commonjs"}"#,
            ),
            (
                "/app/main.js",
                "import { readFileSync } from 'node:fs';\nconsole.log(readFileSync);\n",
            ),
        ]);
        match analyze(Path::new("/app/main.js"), &fs) {
            Err(Error::Format { path, message }) => {
                assert_eq!(path, PathBuf::from("/app/main.js"));
                assert!(message.contains("loaded as CommonJS"), "{message}");
                assert!(message.contains("rename it to .mjs"), "{message}");
                assert!(message.contains("\"type\": \"module\""), "{message}");
                assert!(
                    message.contains("The analyzer does not run loader hooks"),
                    "{message}"
                );
            }
            other => panic!("expected a format error, got {other:?}"),
        }
    }

    #[test]
    fn a_js_file_loaded_as_esm_cannot_use_commonjs_syntax() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "module"}"#),
            ("/app/main.js", "return;\n"),
        ]);
        match analyze(Path::new("/app/main.js"), &fs) {
            Err(Error::Format { message, .. }) => {
                assert!(message.contains("loaded as an ES module"), "{message}");
                assert!(message.contains("remove \"type\": \"module\""), "{message}");
                assert!(
                    message.contains("The analyzer does not run loader hooks"),
                    "{message}"
                );
            }
            other => panic!("expected a format error, got {other:?}"),
        }
    }

    #[test]
    fn a_cjs_file_keeps_the_parser_message_for_module_syntax() {
        let fs = MemoryFs::new(&[("/app/main.cjs", "import 'node:fs';\n")]);
        match analyze(Path::new("/app/main.cjs"), &fs) {
            Err(Error::Parse { path, message }) => {
                assert_eq!(path, PathBuf::from("/app/main.cjs"));
                assert!(message.contains("import"), "{message}");
            }
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn missing_optional_dependency_only_warns() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "try { require('optional-dep'); } catch {}\nrequire('./lib.js');\n",
            ),
            ("/app/lib.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(
            analysis
                .warnings
                .iter()
                .any(|warning| warning.message.contains("optional-dep"))
        );
        assert_eq!(analysis.files.len(), 2);

        let required = MemoryFs::new(&[("/app/main.js", "require('required-dep');\n")]);
        assert!(analyze(Path::new("/app/main.js"), &required).is_err());
    }

    #[test]
    fn a_missing_dependency_under_a_guarded_import_only_warns() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.mjs",
                "try { await import('./feature.mjs'); } catch (error) { if (error.code !== 'ERR_MODULE_NOT_FOUND') throw error; }\n",
            ),
            ("/app/feature.mjs", "import './nested.mjs';\n"),
            (
                "/app/nested.mjs",
                "import 'optional-package-not-installed';\n",
            ),
        ]);
        let analysis = analyze(Path::new("/app/main.mjs"), &fs).unwrap();
        for path in ["/app/feature.mjs", "/app/nested.mjs"] {
            assert!(analysis.files.contains(&PathBuf::from(path)), "{path}");
            assert!(!analysis.required.contains(&PathBuf::from(path)), "{path}");
        }
        assert!(
            analysis.warnings.iter().any(|warning| warning
                .message
                .contains("optional dependency `optional-package-not-installed` is missing")),
            "{:?}",
            analysis.warnings
        );
    }

    #[test]
    fn a_missing_dependency_under_an_unhandled_import_fails() {
        let fs = MemoryFs::new(&[
            ("/app/main.mjs", "await import('./feature.mjs');\n"),
            (
                "/app/feature.mjs",
                "import 'optional-package-not-installed';\n",
            ),
        ]);
        match analyze(Path::new("/app/main.mjs"), &fs) {
            Err(Error::Resolve { specifier, .. }) => {
                assert_eq!(specifier, "optional-package-not-installed");
            }
            other => panic!("expected a resolve error, got {other:?}"),
        }
    }

    #[test]
    fn a_required_edge_keeps_a_shared_file_required() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "try { require('./optional-path'); } catch {}\nrequire('./required-path');\n",
            ),
            ("/app/optional-path.js", "require('./feature');\n"),
            ("/app/required-path.js", "require('./feature');\n"),
            (
                "/app/feature.js",
                "require('optional-package-not-installed');\n",
            ),
        ]);
        match analyze(Path::new("/app/main.js"), &fs) {
            Err(Error::Resolve { specifier, .. }) => {
                assert_eq!(specifier, "optional-package-not-installed");
            }
            other => panic!("expected a resolve error, got {other:?}"),
        }
    }

    #[test]
    fn files_reached_only_through_optional_edges_are_optional() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "try { require('./guarded'); } catch {}\nrequire('./needed');\n",
            ),
            (
                "/app/guarded/index.js",
                "module.exports = require('./nested');\n",
            ),
            ("/app/guarded/nested.js", "module.exports = 1;\n"),
            ("/app/needed.js", "module.exports = 2;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(analysis.required.contains(&PathBuf::from("/app/main.js")));
        assert!(analysis.required.contains(&PathBuf::from("/app/needed.js")));
        assert!(
            !analysis
                .required
                .contains(&PathBuf::from("/app/guarded/index.js"))
        );
        // The whole subtree keeps the optionality of the edge that reached it.
        assert!(
            !analysis
                .required
                .contains(&PathBuf::from("/app/guarded/nested.js"))
        );
        assert!(
            analysis
                .files
                .contains(&PathBuf::from("/app/guarded/nested.js"))
        );
    }

    #[test]
    fn a_later_required_edge_beats_an_earlier_optional_one() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "try { require('./a'); } catch {}\nrequire('./b');\n",
            ),
            ("/app/a.js", "try { require('./shared'); } catch {}\n"),
            ("/app/b.js", "require('./shared');\n"),
            ("/app/shared.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(analysis.required.contains(&PathBuf::from("/app/shared.js")));
    }

    #[test]
    fn a_require_in_a_catch_or_finally_block_is_required() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "try { require('./primary'); } catch { require('./fallback'); } finally { require('./cleanup'); }\n",
            ),
            ("/app/primary.js", "module.exports = 1;\n"),
            ("/app/fallback.js", "module.exports = 2;\n"),
            ("/app/cleanup.js", "module.exports = 3;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(
            !analysis
                .required
                .contains(&PathBuf::from("/app/primary.js"))
        );
        assert!(
            analysis
                .required
                .contains(&PathBuf::from("/app/fallback.js"))
        );
        assert!(
            analysis
                .required
                .contains(&PathBuf::from("/app/cleanup.js"))
        );
    }

    #[test]
    fn records_the_manifests_the_loader_reads() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type": "module"}"#),
            ("/app/main.mjs", "require('./lib');\nrequire('./helper');\n"),
            ("/app/helper.js", "module.exports = 1;\n"),
            ("/app/lib/package.json", r#"{"main": "entry.js"}"#),
            ("/app/lib/entry.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.mjs"), &fs).unwrap();
        assert!(
            analysis
                .manifests
                .contains(&PathBuf::from("/app/package.json"))
        );
        assert!(
            analysis
                .manifests
                .contains(&PathBuf::from("/app/lib/package.json"))
        );
        assert_eq!(analysis.manifests.len(), 2);
    }

    #[test]
    fn a_file_reached_both_ways_is_required() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "try { require('./shared'); } catch {}\nrequire('./shared');\n",
            ),
            ("/app/shared.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(analysis.required.contains(&PathBuf::from("/app/shared.js")));
    }

    #[test]
    fn merging_promotes_files_required_by_any_graph() {
        let guarded = MemoryFs::new(&[
            ("/app/main.js", "try { require('./shared'); } catch {}\n"),
            ("/app/shared.js", "module.exports = 1;\n"),
        ]);
        let mut analysis = analyze(Path::new("/app/main.js"), &guarded).unwrap();
        assert!(!analysis.required.contains(&PathBuf::from("/app/shared.js")));

        let needed = MemoryFs::new(&[
            ("/app/main.js", "require('./shared');\n"),
            ("/app/shared.js", "module.exports = 1;\n"),
        ]);
        analysis.merge(analyze(Path::new("/app/main.js"), &needed).unwrap());
        assert!(analysis.required.contains(&PathBuf::from("/app/shared.js")));
    }

    #[test]
    fn shared_roots_are_read_once_without_changing_the_analysis() {
        let fixture = [
            ("/app/package.json", "{}"),
            (
                "/app/main.js",
                "try { require('./shared'); } catch {}\ntry { require('./guarded'); } catch {}\nrequire('./entry-only');\n",
            ),
            ("/app/entry-only.js", "require('node:events');\n"),
            (
                "/app/extra.js",
                "require('./shared');\nrequire('node:fs');\n",
            ),
            (
                "/app/shared.js",
                "require('./leaf');\nrequire('node:path');\n",
            ),
            ("/app/leaf.js", "require('node:os');\n"),
            ("/app/guarded.js", "module.exports = 1;\n"),
        ];
        let roots = [
            PathBuf::from("/app/extra.js"),
            PathBuf::from("/app/shared.js"),
            PathBuf::from("/app/leaf.js"),
        ];

        let separate = MemoryFs::new(&fixture);
        let mut merged = analyze(Path::new("/app/main.js"), &separate).unwrap();
        for root in &roots {
            merged.merge(analyze(root, &separate).unwrap());
        }

        let fs = MemoryFs::new(&fixture);
        let analysis = analyze_roots(Path::new("/app/main.js"), &roots, &fs).unwrap();
        assert_eq!(analysis.files, merged.files);
        assert_eq!(analysis.required, merged.required);
        assert_eq!(analysis.builtins, merged.builtins);
        assert_eq!(analysis.warnings, merged.warnings);
        assert_eq!(capabilities(&analysis), capabilities(&merged));
        for root in [
            "/app/main.js",
            "/app/entry-only.js",
            "/app/extra.js",
            "/app/shared.js",
            "/app/leaf.js",
            "/app/guarded.js",
        ] {
            assert_eq!(
                fs.reads.borrow().get(Path::new(root)),
                Some(&1),
                "{root} was read more than once"
            );
        }
        assert!(analysis.required.contains(&PathBuf::from("/app/shared.js")));
        assert!(
            !analysis
                .required
                .contains(&PathBuf::from("/app/guarded.js"))
        );
    }

    #[test]
    fn warns_about_dynamic_requires() {
        let fs = MemoryFs::new(&[(
            "/app/main.js",
            "const name = process.argv[2];\nrequire(name);\n",
        )]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert_eq!(analysis.warnings.len(), 1);
        assert!(analysis.warnings[0].message.contains("non-literal"));
        assert!(analysis.warnings[0].message.contains("--include"));
        assert!(analysis.warnings[0].strict);
    }

    #[test]
    fn dynamic_import_is_required_unless_guarded() {
        let fs = MemoryFs::new(&[("/app/main.js", "import('./missing.js');\n")]);
        assert!(matches!(
            analyze(Path::new("/app/main.js"), &fs),
            Err(Error::Resolve { .. })
        ));

        let fs = MemoryFs::new(&[("/app/main.js", "try { import('./missing.js'); } catch {}\n")]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(
            analysis
                .warnings
                .iter()
                .any(|warning| warning.message.contains("optional dependency")),
            "{:?}",
            analysis.warnings
        );
    }

    #[test]
    fn a_shadowed_require_is_not_a_dependency() {
        let fs = MemoryFs::new(&[(
            "/app/main.js",
            "(function (require) { return require('./bundle-only.cjs'); })(() => 42);\n",
        )]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert_eq!(analysis.files, vec![PathBuf::from("/app/main.js")]);
    }

    #[test]
    fn detects_esm_syntax_in_a_typeless_js_file() {
        let fs = MemoryFs::new(&[("/app/main.js", "export const answer = 42;\n")]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert_eq!(
            analysis.formats.get(Path::new("/app/main.js")),
            Some(&Format::Esm)
        );
        assert!(
            analysis
                .warnings
                .iter()
                .any(|warning| warning.message.contains("without \"type\"")),
            "{:?}",
            analysis.warnings
        );
    }

    #[test]
    fn an_explicit_commonjs_type_does_not_detect_esm_syntax() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type":"commonjs"}"#),
            ("/app/main.js", "export const answer = 42;\n"),
        ]);
        assert!(matches!(
            analyze(Path::new("/app/main.js"), &fs),
            Err(Error::Format { .. })
        ));
    }

    #[test]
    fn follows_a_function_built_import_target() {
        let factory = "const load = new Function('specifier', 'return import(specifier)');\n";
        for source in [
            format!("{factory}load('./late.mjs');\n"),
            format!(
                "export function run() {{\n  {factory}  return load('./late.mjs');\n}}\nrun();\n"
            ),
        ] {
            let fs = MemoryFs::new(&[
                ("/app/main.mjs", &source),
                ("/app/late.mjs", "export const answer = 42;\n"),
            ]);
            let analysis = analyze(Path::new("/app/main.mjs"), &fs).unwrap();
            assert!(
                analysis.files.contains(&PathBuf::from("/app/late.mjs")),
                "{source}: {:?}",
                analysis.files
            );
            assert!(
                analysis.warnings.is_empty(),
                "{source}: {:?}",
                analysis.warnings
            );
        }
    }

    #[test]
    fn a_function_built_import_with_a_computed_target_warns() {
        for argument in ["process.argv[2]", "...['./late.mjs']"] {
            let source = format!(
                "const load = new Function('specifier', 'return import(specifier)');\nload({argument});\n"
            );
            let fs = MemoryFs::new(&[("/app/main.mjs", &source)]);
            let analysis = analyze(Path::new("/app/main.mjs"), &fs).unwrap();
            let [warning] = analysis.warnings.as_slice() else {
                panic!(
                    "{source}: expected one warning, got {:?}",
                    analysis.warnings
                );
            };
            assert!(warning.strict, "{source}");
            assert!(
                warning.message.contains("`Function`-built import"),
                "{source}"
            );
            assert!(warning.message.contains("--include"), "{source}");
        }
    }

    #[test]
    fn a_function_built_import_that_escapes_warns() {
        let factory = "const load = new Function('specifier', 'return import(specifier)');\n";
        let cases = [
            ("alias", format!("{factory}const alias = load;\nalias('./late.mjs');\n")),
            ("object", format!("{factory}({{ load }}).load('./late.mjs');\n")),
            (
                "return",
                format!(
                    "function get() {{\n  {factory}  return load;\n}}\nget()('./late.mjs');\n"
                ),
            ),
            ("reference", format!("{factory}pass(load);\n")),
            (
                "inline",
                "(new Function('specifier', 'return import(specifier)'))('./late.mjs');\n"
                    .to_owned(),
            ),
            (
                "exported declaration",
                "export const load = new Function('specifier', 'return import(specifier)');\nload('./late.mjs');\n".to_owned(),
            ),
            (
                "exported reference",
                format!("{factory}export {{ load }};\nload('./late.mjs');\n"),
            ),
        ];
        for (case, source) in cases {
            let fs = MemoryFs::new(&[
                ("/app/main.mjs", &source),
                ("/app/late.mjs", "export const answer = 42;\n"),
            ]);
            let analysis = analyze(Path::new("/app/main.mjs"), &fs).unwrap();
            assert!(
                !analysis.files.contains(&PathBuf::from("/app/late.mjs")),
                "{case}: {:?}",
                analysis.files
            );
            assert!(
                analysis
                    .warnings
                    .iter()
                    .any(|warning| warning.strict && warning.message.contains("--include")),
                "{case}: {:?}",
                analysis.warnings
            );
        }
    }

    #[test]
    fn a_guarded_function_built_import_keeps_its_optionality() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.mjs",
                "const load = new Function('specifier', 'return import(specifier)');\ntry { await load('./feature.mjs'); } catch {}\n",
            ),
            ("/app/feature.mjs", "import './nested.mjs';\n"),
            (
                "/app/nested.mjs",
                "import 'optional-package-not-installed';\n",
            ),
        ]);
        let analysis = analyze(Path::new("/app/main.mjs"), &fs).unwrap();
        for path in ["/app/feature.mjs", "/app/nested.mjs"] {
            assert!(analysis.files.contains(&PathBuf::from(path)), "{path}");
            assert!(!analysis.required.contains(&PathBuf::from(path)), "{path}");
        }
        assert!(
            analysis.warnings.iter().any(|warning| warning
                .message
                .contains("optional dependency `optional-package-not-installed` is missing")),
            "{:?}",
            analysis.warnings
        );
    }

    #[test]
    fn warns_about_function_constructed_imports() {
        let fs = MemoryFs::new(&[(
            "/app/main.js",
            "new Function('module', \"return import(module + '.js')\");\n",
        )]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert_eq!(analysis.warnings.len(), 1);
        assert!(analysis.warnings[0].message.contains("--include"));
    }

    #[test]
    fn records_a_static_asset_read_through_dirname() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "const fs = require('node:fs');\nconst path = require('node:path');\nfs.readFileSync(path.join(__dirname, 'data.css'), 'utf8');\n",
            ),
            ("/app/data.css", "body {}\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(
            analysis.data_files.contains(Path::new("/app/data.css")),
            "{:?}",
            analysis.data_files
        );
    }

    #[test]
    fn records_a_declaration_file_named_beside_a_module() {
        let fs = MemoryFs::new(&[
            (
                "/app/compiler.js",
                "const fs = require('node:fs');\nconst libs = ['lib.d.ts', 'helper.js', 'data.json', 'nested/lib.d.ts'];\nfor (const lib of libs) fs.readFileSync(lib);\n",
            ),
            ("/app/lib.d.ts", "/// <reference lib=\"es2020\" />\n"),
            ("/app/helper.js", "module.exports = 42;\n"),
            ("/app/data.json", "{}\n"),
            ("/app/nested/lib.d.ts", "declare const nested: number;\n"),
        ]);
        let analysis = analyze(Path::new("/app/compiler.js"), &fs).unwrap();
        assert_eq!(
            analysis.data_files,
            BTreeSet::from([PathBuf::from("/app/lib.d.ts")])
        );
    }

    #[test]
    fn a_resolve_only_target_is_payload_data_not_a_module() {
        let fs = MemoryFs::new(&[
            ("/app/package.json", r#"{"type":"commonjs"}"#),
            (
                "/app/main.cjs",
                "const tool = require.resolve('./tool');\nconsole.log(tool.endsWith('/tool'));\n",
            ),
            ("/app/tool", "#!/bin/sh\nset -e\necho never-run\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.cjs"), &fs).unwrap();
        assert!(
            analysis.data_files.contains(Path::new("/app/tool")),
            "{:?}",
            analysis.data_files
        );
        assert!(
            !analysis
                .files
                .iter()
                .any(|path| path == Path::new("/app/tool"))
        );
        assert_eq!(fs.reads.borrow().get(Path::new("/app/tool")), None);
    }

    #[test]
    fn static_manifest_names_need_plain_data_and_direct_calls() {
        let load = "const load = (options) => require.resolve(`${options.package}/package.json`);";
        let cases = [
            ("plain", format!("const cli = {{ package: 'tool' }};\n{load}\nload(cli);\n"), false),
            ("object literal argument", format!("{load}\nload({{ package: 'tool' }});\n"), false),
            ("spread", format!("const cli = {{ ...{{ package: 'tool' }} }};\n{load}\nload(cli);\n"), true),
            ("computed key", format!("const cli = {{ ['package']: 'tool' }};\n{load}\nload(cli);\n"), true),
            ("duplicate key", format!("const cli = {{ package: 'old', package: 'tool' }};\n{load}\nload(cli);\n"), true),
            ("getter", format!("const cli = {{ get package() {{ return 'tool'; }} }};\n{load}\nload(cli);\n"), true),
            ("alias write", format!("const cli = {{ package: 'tool' }};\nconst alias = cli;\nalias.package = 'old';\n{load}\nload(cli);\n"), true),
            ("escape", format!("const cli = {{ package: 'tool' }};\nmutate(cli);\n{load}\nload(cli);\n"), true),
            ("parameter reassignment", "const load = (options) => { options = { package: 'tool' }; return require.resolve(`${options.package}/package.json`); };\nload({ package: 'tool' });\n".to_owned(), true),
            ("parameter destructuring", "const load = ({ package: name }) => require.resolve(`${name}/package.json`);\nload({ package: 'tool' });\n".to_owned(), true),
            ("conflicting calls", format!("const cli = {{ package: 'tool' }};\nconst other = {{ package: 'other' }};\n{load}\nload(cli);\nload(other);\n"), true),
            ("escaped function", format!("const cli = {{ package: 'tool' }};\n{load}\nmodule.exports.load = load;\nload(cli);\n"), true),
        ];
        for (name, source, rejected) in cases {
            let fs = MemoryFs::new(&[
                ("/app/package.json", r#"{"type":"commonjs"}"#),
                ("/app/main.cjs", &source),
                (
                    "/app/node_modules/tool/package.json",
                    r#"{"name":"tool","version":"1.0.0"}"#,
                ),
                (
                    "/app/node_modules/other/package.json",
                    r#"{"name":"other","version":"1.0.0"}"#,
                ),
            ]);
            let analysis = analyze(Path::new("/app/main.cjs"), &fs).unwrap();
            let strict = analysis.warnings.iter().any(|warning| warning.strict);
            assert_eq!(strict, rejected, "{name}: {:?}", analysis.warnings);
            let resolved = analysis
                .data_files
                .iter()
                .any(|path| path.ends_with("tool/package.json"));
            assert_eq!(resolved, !rejected, "{name}: {:?}", analysis.data_files);
        }
    }

    #[test]
    fn a_resolve_only_target_is_still_parsed_when_also_loaded() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.cjs",
                "console.log(require.resolve('./tool'));\nrequire('./tool');\n",
            ),
            ("/app/tool", "#!/bin/sh\nset -e\necho never-run\n"),
        ]);
        assert!(matches!(
            analyze(Path::new("/app/main.cjs"), &fs),
            Err(Error::Parse { .. })
        ));
    }

    #[test]
    fn an_extensionless_javascript_module_still_loads() {
        let fs = MemoryFs::new(&[
            ("/app/main.cjs", "console.log(require('./tool'));\n"),
            ("/app/tool", "#!/usr/bin/env node\nmodule.exports = 42;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.cjs"), &fs).unwrap();
        assert!(
            analysis
                .files
                .iter()
                .any(|path| path == Path::new("/app/tool")),
            "{:?}",
            analysis.files
        );
    }

    #[test]
    fn a_leading_slash_in_a_joined_asset_stays_under_dirname() {
        let fs = MemoryFs::new(&[
            (
                "/app/nested/main.cjs",
                "const fs = require('node:fs');\nconst path = require('node:path');\nfs.readFileSync(path.join(__dirname, '/public/style.css'), 'utf8');\n",
            ),
            ("/app/nested/public/style.css", "body {}\n"),
        ]);
        let analysis = analyze(Path::new("/app/nested/main.cjs"), &fs).unwrap();
        assert_eq!(
            analysis.data_files,
            BTreeSet::from([PathBuf::from("/app/nested/public/style.css")])
        );
        assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
    }

    #[test]
    fn a_resolved_asset_still_resets_at_a_leading_slash() {
        // Control for the join case: `path.resolve` drops the module directory.
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "const fs = require('node:fs');\nconst path = require('node:path');\nfs.readFileSync(path.resolve(__dirname, '/public/style.css'), 'utf8');\n",
            ),
            ("/public/style.css", "body {}\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert_eq!(
            analysis.data_files,
            BTreeSet::from([PathBuf::from("/public/style.css")])
        );
        assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
    }

    #[test]
    fn a_concatenated_asset_keeps_its_leading_slash_under_dirname() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "const fs = require('node:fs');\nfs.readFileSync(__dirname + '/data.css', 'utf8');\n",
            ),
            ("/app/data.css", "body {}\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert_eq!(
            analysis.data_files,
            BTreeSet::from([PathBuf::from("/app/data.css")])
        );
        assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
    }

    #[test]
    fn a_dynamic_asset_path_is_left_alone() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.js",
                "const fs = require('node:fs');\nconst path = require('node:path');\nfs.readFileSync(path.join(__dirname, name), 'utf8');\n",
            ),
            ("/app/data.css", "body {}\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(analysis.data_files.is_empty());
        assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
    }

    #[test]
    fn records_a_static_directory_scanned_through_dirname() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.cjs",
                "const fs = require('node:fs');\nconst dir = __dirname + '/plugins';\nfs.readdirSync(dir);\n",
            ),
            ("/app/plugins/plugin.cjs", "module.exports = 42;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.cjs"), &fs).unwrap();
        assert_eq!(
            analysis.static_dirs,
            BTreeMap::from([(PathBuf::from("/app/plugins"), StaticDir::All)])
        );
    }

    #[test]
    fn a_computed_module_load_records_its_candidate_directory() {
        let fs = MemoryFs::new(&[
            (
                "/app/lib/main.cjs",
                "const path = require('node:path');\n\
                 const { pathToFileURL } = require('node:url');\n\
                 async function load(name) {\n\
                   let formatterPath = path.resolve(process.cwd(), name);\n\
                   formatterPath = path.resolve(__dirname, '../', 'formatters', `${name}.js`);\n\
                   return import(pathToFileURL(formatterPath));\n\
                 }\n\
                 console.log(load);\n",
            ),
            ("/app/formatters/stylish.js", "module.exports = () => '';\n"),
            ("/app/formatters/meta.json", "[]\n"),
        ]);
        let analysis = analyze(Path::new("/app/lib/main.cjs"), &fs).unwrap();
        let filter = &analysis.static_dirs[Path::new("/app/formatters")];
        assert!(filter.accepts(Path::new("/app/formatters/stylish.js")));
        assert!(!filter.accepts(Path::new("/app/formatters/meta.json")));
        assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
    }

    #[test]
    fn direct_eval_keeps_a_computed_manifest_dynamic() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.cjs",
                r#"const paths = { package: "old" };
                   eval('paths.package = "needed"');
                   require.resolve(`${paths.package}/package.json`);"#,
            ),
            ("/app/node_modules/old/package.json", r#"{"name":"old"}"#),
        ]);
        let result = analyze(Path::new("/app/main.cjs"), &fs).unwrap();
        assert!(result.data_files.is_empty());
        assert!(result.warnings.iter().any(|warning| warning.strict));
    }

    #[test]
    fn a_bin_selection_does_not_leak_to_another_importer() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.cjs",
                r#"const path = require("node:path");
                   const name = require.resolve("tool/package.json");
                   const pkg = require(name);
                   require(path.resolve(path.dirname(name), pkg.bin.tool));
                   require("./viewer/view.cjs");"#,
            ),
            (
                "/app/node_modules/tool/package.json",
                r#"{"name":"tool","bin":{"tool":"entry.cjs"}}"#,
            ),
            ("/app/node_modules/tool/entry.cjs", "module.exports = 1;"),
            (
                "/app/viewer/view.cjs",
                r#"require.resolve("tool/package.json");"#,
            ),
            (
                "/app/viewer/node_modules/tool/package.json",
                r#"{"name":"tool","bin":{"tool":"entry.cjs"}}"#,
            ),
            (
                "/app/viewer/node_modules/tool/entry.cjs",
                "module.exports = 2;",
            ),
        ]);
        let result = analyze(Path::new("/app/main.cjs"), &fs).unwrap();
        assert!(
            result
                .files
                .contains(&PathBuf::from("/app/node_modules/tool/entry.cjs"))
        );
        assert!(
            !result
                .files
                .contains(&PathBuf::from("/app/viewer/node_modules/tool/entry.cjs"))
        );
        assert!(
            result
                .data_files
                .contains(Path::new("/app/viewer/node_modules/tool/package.json"))
        );
    }

    #[test]
    fn follows_a_require_of_a_resolved_constant_path() {
        let fs = MemoryFs::new(&[
            (
                "/app/lib/cli.js",
                "const path = require('node:path');\nconst cliEntry = path.resolve(__dirname, 'cli/entry.js');\nmodule.exports = () => require(cliEntry);\n",
            ),
            ("/app/lib/cli/entry.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/lib/cli.js"), &fs).unwrap();
        assert!(
            analysis
                .files
                .contains(&PathBuf::from("/app/lib/cli/entry.js")),
            "{:?}",
            analysis.files
        );
        assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
    }

    #[cfg(windows)]
    #[test]
    fn follows_a_require_of_a_resolved_constant_path_on_a_windows_host() {
        let fs = MemoryFs::new(&[
            (
                "C:\\app\\lib\\cli.js",
                "const path = require('node:path');\nconst cliEntry = path.resolve(__dirname, 'cli/entry.js');\nmodule.exports = () => require(cliEntry);\n",
            ),
            ("C:\\app\\lib\\cli\\entry.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("C:\\app\\lib\\cli.js"), &fs).unwrap();
        assert!(
            analysis
                .files
                .contains(&PathBuf::from("C:\\app\\lib\\cli\\entry.js")),
            "{:?}",
            analysis.files
        );
        assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
    }

    #[test]
    fn a_require_path_resolved_from_the_working_directory_is_left_alone() {
        // Control for the rooted case: `path.resolve('cli/entry.js')` is relative to
        // the process working directory, which packing does not know.
        let fs = MemoryFs::new(&[
            (
                "/app/lib/cli.js",
                "const path = require('node:path');\nrequire(path.resolve('cli/entry.js'));\n",
            ),
            ("/app/lib/cli/entry.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/lib/cli.js"), &fs).unwrap();
        assert!(
            !analysis
                .files
                .contains(&PathBuf::from("/app/lib/cli/entry.js")),
            "{:?}",
            analysis.files
        );
        assert_eq!(analysis.warnings.len(), 1);
        assert!(analysis.warnings[0].message.contains("non-literal"));
    }

    #[test]
    fn a_direct_template_module_load_records_its_candidate_directory() {
        let fs = MemoryFs::new(&[
            (
                "/app/lib/npm.js",
                "const command = process.argv[2];\nmodule.exports = () => require(`./commands/${command}.js`);\n",
            ),
            ("/app/lib/commands/pkg.js", "module.exports = 1;\n"),
            ("/app/lib/commands/README.md", "not a command\n"),
        ]);
        let analysis = analyze(Path::new("/app/lib/npm.js"), &fs).unwrap();
        let filter = &analysis.static_dirs[Path::new("/app/lib/commands")];
        assert!(filter.accepts(Path::new("/app/lib/commands/pkg.js")));
        assert!(!filter.accepts(Path::new("/app/lib/commands/README.md")));
        assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
    }

    #[test]
    fn a_direct_template_without_a_literal_extension_keeps_its_warning() {
        // Control for the bounded directory: only a literal extension bounds the
        // candidates the template can name.
        let fs = MemoryFs::new(&[
            (
                "/app/lib/npm.js",
                "const command = process.argv[2];\nrequire(`./commands/${command}`);\n",
            ),
            ("/app/lib/commands/pkg.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/lib/npm.js"), &fs).unwrap();
        assert!(
            analysis.static_dirs.is_empty(),
            "{:?}",
            analysis.static_dirs
        );
        assert_eq!(analysis.warnings.len(), 1);
        assert!(analysis.warnings[0].message.contains("non-literal"));
    }

    #[test]
    fn a_resolve_only_direct_template_keeps_its_warning() {
        // `require.resolve` only locates its target, so the candidate directory is
        // not a module subtree to pack.
        let fs = MemoryFs::new(&[
            (
                "/app/lib/npm.js",
                "const command = process.argv[2];\nmodule.exports = require.resolve(`./commands/${command}.js`);\n",
            ),
            ("/app/lib/commands/pkg.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/lib/npm.js"), &fs).unwrap();
        assert!(
            analysis.static_dirs.is_empty(),
            "{:?}",
            analysis.static_dirs
        );
        assert_eq!(analysis.warnings.len(), 1);
        assert!(analysis.warnings[0].message.contains("non-literal"));
    }

    #[test]
    fn a_guarded_direct_template_keeps_its_warning() {
        // A guarded load may be absent at run time; packing the directory as a
        // required include root would promote it to a required subtree.
        let fs = MemoryFs::new(&[
            (
                "/app/lib/npm.js",
                "const command = process.argv[2];\ntry { require(`./commands/${command}.js`); } catch {}\n",
            ),
            ("/app/lib/commands/pkg.js", "module.exports = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/lib/npm.js"), &fs).unwrap();
        assert!(
            analysis.static_dirs.is_empty(),
            "{:?}",
            analysis.static_dirs
        );
        assert_eq!(analysis.warnings.len(), 1);
        assert!(analysis.warnings[0].message.contains("non-literal"));
    }

    #[test]
    fn a_computed_module_load_without_a_literal_dir_keeps_its_warning() {
        let fs = MemoryFs::new(&[(
            "/app/lib/main.cjs",
            "const path = require('node:path');\n\
             const name = process.argv[2];\n\
             require(path.resolve(process.cwd(), name));\n",
        )]);
        let analysis = analyze(Path::new("/app/lib/main.cjs"), &fs).unwrap();
        assert!(analysis.static_dirs.is_empty());
        assert_eq!(analysis.warnings.len(), 1);
        assert!(analysis.warnings[0].message.contains("non-literal"));
    }

    #[test]
    fn a_dynamic_directory_scan_is_left_alone() {
        let fs = MemoryFs::new(&[
            (
                "/app/main.cjs",
                "const fs = require('node:fs');\nfs.readdirSync(__dirname + '/' + process.argv[2]);\n",
            ),
            ("/app/plugins/plugin.cjs", "module.exports = 42;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.cjs"), &fs).unwrap();
        assert!(
            analysis.static_dirs.is_empty(),
            "{:?}",
            analysis.static_dirs
        );
    }

    #[test]
    fn an_unrelated_readdir_is_left_alone() {
        // Only `fs` enumerates files the payload carries; another object's
        // `readdirSync` returns whatever it likes.
        let fs = MemoryFs::new(&[
            (
                "/app/main.cjs",
                "const scanner = { readdirSync: () => ['plugin.cjs'] };\nscanner.readdirSync(__dirname + '/plugins');\n",
            ),
            ("/app/plugins/plugin.cjs", "module.exports = 42;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.cjs"), &fs).unwrap();
        assert!(
            analysis.static_dirs.is_empty(),
            "{:?}",
            analysis.static_dirs
        );
    }

    #[test]
    fn detects_websocket_global() {
        let fs = MemoryFs::new(&[("/app/main.js", "new WebSocket('ws://127.0.0.1:1');\n")]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        let capabilities = capabilities(&analysis);
        for name in [
            "fetch", "http", "net", "tls", "crypto", "buffer", "stream", "zlib",
        ] {
            assert!(
                capabilities.iter().any(|capability| capability == name),
                "missing {name}: {capabilities:?}"
            );
        }
    }

    #[test]
    fn follows_esbuild_require_wrapper() {
        let source = include_str!("../../../tests/esbuild/require-wrapper.mjs");
        let fs = MemoryFs::new(&[("/app/main.mjs", source)]);
        let analysis = analyze(Path::new("/app/main.mjs"), &fs).unwrap();
        assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
        assert!(analysis.builtins.contains_key("tty"));
        assert!(analysis.builtins.contains_key("constants"));
    }

    #[test]
    fn follows_require_aliases_and_minified_wrappers() {
        let cases = [
            ("const load = require;\nload('node:tty');\n", "tty"),
            (
                "function startup() { r.resolve('constants'); } var r = (e => typeof require < 'u' ? require : e)(null); startup();",
                "constants",
            ),
            (
                "var r = (function(e) { return typeof require < 'u' ? require : e; })(null); r('tty');",
                "tty",
            ),
        ];
        for (source, expected) in cases {
            let fs = MemoryFs::new(&[("/app/main.js", source)]);
            let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
            let capabilities = capabilities(&analysis);
            assert!(
                capabilities.iter().any(|capability| capability == expected),
                "{source} did not pull {expected}: {capabilities:?}"
            );
        }
    }

    #[test]
    fn a_local_require_lookalike_is_not_a_loader() {
        for source in [
            "function __require(specifier) { return specifier; }\n__require('node:tty');\n",
            "const __require = (specifier) => [require, specifier]; __require('node:tty');",
            "const __require = require; (__require => __require('node:tty'))(x => x);",
            "function wrap(require) { const load = require; load('node:tty'); } wrap(x => x);",
            "function wrap(require) { const load = (e => typeof require !== 'undefined' ? require : e)(null); load('node:tty'); } wrap(x => x);",
            "let __require = require; __require = x => x; __require('node:tty');",
        ] {
            let fs = MemoryFs::new(&[("/app/main.js", source)]);
            let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
            assert!(
                !analysis.builtins.contains_key("tty"),
                "{source} pulled tty: {:?}",
                analysis.builtins
            );
        }
    }

    #[test]
    fn warns_about_dynamic_wrapper_requests() {
        let source = "var load = (e => typeof require !== 'undefined' ? require : e)(null); load(process.argv[2]);";
        let fs = MemoryFs::new(&[("/app/main.cjs", source)]);
        let analysis = analyze(Path::new("/app/main.cjs"), &fs).unwrap();
        let [warning] = analysis.warnings.as_slice() else {
            panic!("expected one warning, got {:?}", analysis.warnings);
        };
        assert!(warning.strict);
        assert!(warning.message.contains("`load(...)`"));
        assert!(warning.message.contains("`--with <module>`"));
    }

    #[test]
    fn detects_fetch_globals() {
        let cases = [
            "fetch('http://127.0.0.1/');\n",
            "globalThis.fetch('http://127.0.0.1/');\n",
            "new Headers();\n",
            "new Request('http://127.0.0.1/');\n",
            "new Response(null);\n",
            "new FormData();\n",
            "Response.json({});\n",
            "globalThis.Response.redirect('/x');\n",
            "const request = fetch;\nrequest('http://127.0.0.1/');\n",
            "const Socket = WebSocket;\nconsole.log(Socket.name);\n",
            "const { fetch: request } = globalThis;\nrequest('http://127.0.0.1/');\n",
        ];
        for source in cases {
            let fs = MemoryFs::new(&[("/app/main.js", source)]);
            let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
            let capabilities = capabilities(&analysis);
            for name in [
                "fetch", "http", "net", "tls", "crypto", "buffer", "stream", "zlib",
            ] {
                assert!(
                    capabilities.iter().any(|capability| capability == name),
                    "{source} did not pull {name}: {capabilities:?}"
                );
            }
            for name in ["http2", "sqlite"] {
                assert!(
                    !capabilities.iter().any(|capability| capability == name),
                    "{source} pulled {name}: {capabilities:?}"
                );
            }
        }
    }

    #[test]
    fn fetch_selects_the_capability_and_its_closure() {
        let fs = MemoryFs::new(&[("/app/main.js", "fetch('http://127.0.0.1/');\n")]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(
            analysis.builtins.contains_key("thinpack/internal/undici"),
            "missing the fetch marker: {:?}",
            analysis.builtins
        );
        // `http2` and `sqlite` stay out: undici guards both, so a fetch app must pack
        // without them (and `--without http2`/`--without sqlite` must keep working).
        for builtin in ["http2", "sqlite"] {
            assert!(
                !analysis.builtins.contains_key(builtin),
                "pulled {builtin}: {:?}",
                analysis.builtins
            );
        }
        let capabilities = capabilities(&analysis);
        assert_eq!(capabilities.len(), 20);
        assert!(capabilities.iter().any(|name| name == "fetch"));
    }

    #[test]
    fn an_http_only_app_does_not_pull_fetch() {
        let fs = MemoryFs::new(&[(
            "/app/main.js",
            "const http = require('node:http');\nhttp.createServer();\n",
        )]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        let capabilities = capabilities(&analysis);
        assert!(capabilities.iter().any(|name| name == "http"));
        assert!(!capabilities.iter().any(|name| name == "fetch"));
        assert!(
            !analysis.builtins.contains_key("thinpack/internal/undici"),
            "HTTP pulled Undici: {:?}",
            analysis.builtins
        );
    }

    #[test]
    fn shadowed_fetch_globals_do_not_pull_fetch() {
        let cases = [
            "function fetch() { return 1; }\nfetch();\n",
            "const fetch = () => 1;\nfetch();\n",
            "function connect(WebSocket) { return new WebSocket(); }\nconnect(class {});\n",
            "class Response {};\nnew Response();\n",
            "const globalThis = { fetch() { return 1; } };\nglobalThis.fetch();\n",
            "const globalThis = { fetch: 1 };\nconst { fetch } = globalThis;\nconsole.log(fetch);\n",
        ];
        for source in cases {
            let fs = MemoryFs::new(&[("/app/main.js", source)]);
            let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
            let capabilities = capabilities(&analysis);
            assert!(
                !capabilities.iter().any(|name| name == "fetch"),
                "{source} pulled fetch: {capabilities:?}"
            );
        }
    }

    #[test]
    fn report_error_pulls_no_builtin() {
        let fs = MemoryFs::new(&[("/app/main.js", "reportError(new Error('x'));\n")]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(analysis.builtins.is_empty(), "{:?}", analysis.builtins);
    }

    #[test]
    fn detects_builtins_fetched_by_process_get_builtin_module() {
        let fs = MemoryFs::new(&[(
            "/app/main.js",
            "const sea = process.getBuiltinModule('node:sea');\n",
        )]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert!(
            analysis.builtins.contains_key("sea"),
            "{:?}",
            analysis.builtins
        );
    }

    #[test]
    fn optional_builtin_is_required_outside_try() {
        let probed =
            MemoryFs::new(&[("/app/main.js", "try { require('node:http2'); } catch {}\n")]);
        let analysis = analyze(Path::new("/app/main.js"), &probed).unwrap();
        assert!(!analysis.builtins.contains_key("http2"));
        assert!(
            analysis
                .warnings
                .iter()
                .any(|warning| warning.message.contains("optional builtin"))
        );

        let required = MemoryFs::new(&[("/app/main.js", "require('node:http2');\n")]);
        let analysis = analyze(Path::new("/app/main.js"), &required).unwrap();
        assert!(analysis.builtins.contains_key("http2"));
    }

    #[test]
    fn rejects_unknown_builtins() {
        let fs = MemoryFs::new(&[("/app/main.js", "require('node:nope');\n")]);
        let error = analyze(Path::new("/app/main.js"), &fs).unwrap_err();
        assert!(matches!(error, Error::UnknownBuiltin { .. }));
    }
}
