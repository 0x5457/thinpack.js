mod require;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    CallExpression, ExportAllDeclaration, ExportFromDeclaration, Expression, IdentifierReference,
    ImportDeclaration, ImportExpression, NewExpression, TryStatement,
};

use oxc_ast_visit::{VisitJs, walk_js};
use oxc_parser::Parser;
use oxc_semantic::{Semantic, SemanticBuilder, SymbolId};
use oxc_span::SourceType;
use thinpack_resolve::FileSystem;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Warning {
    pub message: String,
    pub strict: bool,
}

#[derive(Debug)]
pub struct Analysis {
    pub files: Vec<PathBuf>,
    pub builtins: BTreeMap<String, BTreeSet<PathBuf>>,
    pub warnings: Vec<Warning>,
}

impl Analysis {
    pub fn merge(&mut self, other: Self) {
        self.files.extend(other.files);
        self.files.sort();
        self.files.dedup();
        for (builtin, files) in other.builtins {
            self.builtins.entry(builtin).or_default().extend(files);
        }
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
            Self::Read { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::Parse { path, message } => {
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
/// when a module does not parse, [`Error::Resolve`] for unresolvable non-optional
/// specifiers and [`Error::UnknownBuiltin`] for `node:` builtins that thinpack
/// does not ship.
pub fn analyze(entry: &Path, fs: &impl FileSystem) -> Result<Analysis, Error> {
    let mut pending = VecDeque::from([entry.to_path_buf()]);
    let mut visited = BTreeSet::new();
    let mut builtins: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut warnings = Vec::new();

    while let Some(path) = pending.pop_front() {
        if !visited.insert(path.clone()) {
            continue;
        }
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            continue;
        }

        let bytes = fs.read(&path).map_err(|error| Error::Read {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let source = String::from_utf8(bytes).map_err(|_| Error::Read {
            path: path.clone(),
            message: "module is not valid UTF-8".to_owned(),
        })?;

        let specifiers = parse_specifiers(&source, &path, &mut warnings)?;
        for ((specifier, is_import), optional) in specifiers {
            if let Some(module) = thinpack_caps::module_for_builtin(&specifier) {
                if module.optional && optional {
                    warnings.push(Warning {
                        message: format!(
                            "{}: optional builtin `{specifier}` may be absent; the app must handle ERR_UNKNOWN_BUILTIN_MODULE",
                            path.display()
                        ),
                        strict: false,
                    });
                    continue;
                }
                let builtin = specifier
                    .strip_prefix("node:")
                    .unwrap_or(&specifier)
                    .to_owned();
                builtins.entry(builtin).or_default().insert(path.clone());
                continue;
            }
            if specifier.starts_with("node:") {
                // A guarded `require` or a `process.getBuiltinModule` probe is
                // how an app checks for a builtin that may not ship; warn like
                // an optional dependency instead of failing the pack.
                if optional {
                    warnings.push(Warning {
                        message: format!(
                            "{}: unknown builtin `{specifier}` may be absent; the app must handle ERR_UNKNOWN_BUILTIN_MODULE",
                            path.display()
                        ),
                        strict: false,
                    });
                    continue;
                }
                return Err(Error::UnknownBuiltin {
                    specifier,
                    from: path,
                });
            }
            let resolved = if is_import {
                thinpack_resolve::resolve_import_path(&specifier, &path, fs)
            } else {
                thinpack_resolve::resolve_path(&specifier, &path, fs)
            };
            let resolved = match resolved {
                Ok(resolved) => resolved,
                Err(error)
                    if optional || thinpack_resolve::declares_optional(&specifier, &path, fs) =>
                {
                    warnings.push(Warning {
                        message: format!(
                            "{}: optional dependency `{specifier}` is missing ({error}); the app must handle the error",
                            path.display()
                        ),
                        strict: false,
                    });
                    continue;
                }
                Err(error) => {
                    return Err(Error::Resolve {
                        specifier,
                        from: path.clone(),
                        message: error.to_string(),
                    });
                }
            };
            pending.push_back(resolved);
        }
    }

    Ok(Analysis {
        files: visited.into_iter().collect(),
        builtins,
        warnings,
    })
}

type SpecifierEntry = ((String, bool), bool);

fn parse_specifiers(
    source: &str,
    path: &Path,
    warnings: &mut Vec<Warning>,
) -> Result<Vec<SpecifierEntry>, Error> {
    let allocator = Allocator::default();
    // `.cjs` parses as CommonJS (top-level `return` and `new.target` are legal)
    // and `.mjs` as an ES module. A generic `.js` is either, so when the first
    // attempt — unambiguous for `.js`, ESM for unknown extensions — rejects the
    // source, retry it as CommonJS; files that are neither still fail.
    let (source_type, may_retry) = SourceType::from_path(path).map_or_else(
        |_| (SourceType::default(), true),
        |source_type| (source_type, source_type.is_unambiguous()),
    );
    let mut parsed = Parser::new(&allocator, source, source_type).parse();
    if may_retry && !parsed.diagnostics.is_empty() {
        let commonjs = Parser::new(&allocator, source, SourceType::cjs()).parse();
        if commonjs.diagnostics.is_empty() {
            parsed = commonjs;
        }
    }
    if !parsed.diagnostics.is_empty() {
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
    let mut collector = SpecifierCollector {
        path,
        specifiers: BTreeMap::new(),
        try_depth: 0,
        semantic: &semantic,
        require_bindings: require::bindings(&semantic),
        warnings,
    };
    collector.visit_program(&parsed.program);
    Ok(collector.specifiers.into_iter().collect())
}

fn js_string(value: oxc_str::JSStr<'_>) -> String {
    value
        .as_str()
        .map_or_else(|| value.display().to_string(), str::to_owned)
}

/// The modules behind a global that the runtime installs lazily, so a static
/// reference to the global is what brings them into the payload.
const WEBSOCKET_BUILTINS: [&str; 4] = ["http", "https", "crypto", "buffer"];
/// The vendored undici bundle (`crates/node/http/vendor/undici.js`) requires these, so a
/// static reference to one of its globals has to pull all of them into the payload.
/// `http2` and `sqlite` are not here: the bundle probes both and handles their absence.
const FETCH_BUILTINS: [&str; 16] = [
    "assert",
    "async_hooks",
    "buffer",
    "crypto",
    "diagnostics_channel",
    "events",
    "http",
    "net",
    "perf_hooks",
    "querystring",
    "stream",
    "tls",
    "url",
    "util",
    "worker_threads",
    "zlib",
];
/// The globals the bundle sits behind. A static reference to any of them — `fetch(...)`,
/// `new Response(...)`, `Response.json(...)` — is what pulls [`FETCH_BUILTINS`].
const FETCH_GLOBALS: [&str; 5] = ["fetch", "Headers", "Request", "Response", "FormData"];

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

/// The global an expression references: bare, through `globalThis`, or as the receiver of a
/// static call, so `fetch(...)`, `globalThis.fetch(...)`, `Response.json(...)` and
/// `globalThis.Response.redirect(...)` all name one.
fn referenced_global<'a>(expression: &'a Expression<'_>) -> Option<&'a str> {
    if let Some(name) = global_name(expression) {
        return Some(name);
    }
    match expression {
        Expression::StaticMemberExpression(member) => global_name(&member.object),
        _ => None,
    }
}

struct SpecifierCollector<'a, 's> {
    path: &'s Path,
    specifiers: BTreeMap<(String, bool), bool>,
    try_depth: usize,
    semantic: &'s Semantic<'a>,
    require_bindings: BTreeSet<SymbolId>,
    warnings: &'s mut Vec<Warning>,
}

impl SpecifierCollector<'_, '_> {
    fn add_specifier(&mut self, specifier: String, is_import: bool) {
        self.add(specifier, is_import, self.try_depth > 0);
    }

    fn add_optional_specifier(&mut self, specifier: String, is_import: bool) {
        self.add(specifier, is_import, true);
    }

    fn add(&mut self, specifier: String, is_import: bool, optional: bool) {
        self.specifiers
            .entry((specifier, is_import))
            .and_modify(|existing| *existing &= optional)
            .or_insert(optional);
    }

    fn add_builtins(&mut self, builtins: &[&str]) {
        for builtin in builtins {
            self.add_specifier((*builtin).to_owned(), false);
        }
    }

    fn is_require_binding(&self, identifier: &IdentifierReference<'_>) -> bool {
        identifier.name == "require"
            || require::referenced_symbol(identifier, self.semantic.scoping())
                .is_some_and(|symbol| self.require_bindings.contains(&symbol))
    }

    fn require_call(&mut self, call: &CallExpression<'_>) {
        // (receiver or callee, member suffix, resolves as import, never throws in Node)
        let target = match call.callee.without_parentheses() {
            Expression::Identifier(identifier) if self.is_require_binding(identifier) => {
                Some((identifier.name.as_str(), "", false, false))
            }
            Expression::StaticMemberExpression(member) => match &member.object {
                Expression::ImportMeta(_) if member.property.name == "resolve" => {
                    Some(("import.meta", ".resolve", true, false))
                }
                Expression::Identifier(object) => {
                    match (object.name.as_str(), member.property.name.as_str()) {
                        (name, "resolve") if self.is_require_binding(object) => {
                            Some((name, ".resolve", false, false))
                        }
                        ("module", "require") => Some(("module", ".require", false, false)),
                        // `getBuiltinModule` returns `null` for a missing module
                        // instead of throwing; a failed lookup is a warning,
                        // not a pack error.
                        ("process", "getBuiltinModule") => {
                            Some(("process", ".getBuiltinModule", false, true))
                        }
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => None,
        };
        let Some((target, member, is_import, optional)) = target else {
            return;
        };

        let Some(argument) = call.arguments.first() else {
            return;
        };
        let complete = argument.as_expression().is_some_and(|argument| {
            require::specifiers(argument, self.semantic, |specifier| {
                self.add(
                    js_string(specifier),
                    is_import,
                    optional || self.try_depth > 0,
                );
            })
        });
        if !complete {
            self.warnings.push(Warning {
                message: format!(
                    "{}: `{target}{member}(...)` with a non-literal argument cannot be analyzed. Add any required builtin capabilities with `--with <module>`.",
                    self.path.display()
                ),
                strict: true,
            });
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
        match &expression.source {
            Expression::StringLiteral(literal) => {
                self.add_optional_specifier(js_string(literal.value), true);
            }
            _ => self.warnings.push(Warning {
                message: format!(
                    "{}: `import(...)` with a non-literal argument cannot be analyzed",
                    self.path.display()
                ),
                strict: true,
            }),
        }
        walk_js::walk_import_expression(self, expression);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        self.require_call(call);
        if referenced_global(&call.callee).is_some_and(|name| FETCH_GLOBALS.contains(&name)) {
            self.add_builtins(&FETCH_BUILTINS);
        }
        walk_js::walk_call_expression(self, call);
    }

    fn visit_try_statement(&mut self, statement: &TryStatement<'a>) {
        self.try_depth += 1;
        walk_js::walk_try_statement(self, statement);
        self.try_depth -= 1;
    }

    fn visit_new_expression(&mut self, expression: &NewExpression<'a>) {
        match global_name(&expression.callee) {
            Some("WebSocket") => self.add_builtins(&WEBSOCKET_BUILTINS),
            Some(name) if FETCH_GLOBALS.contains(&name) => self.add_builtins(&FETCH_BUILTINS),
            _ => {}
        }
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
            (
                "/app/main.mjs",
                "import './a.js';\nexport * from './b.js';\n",
            ),
            ("/app/a.js", "export const a = 1;\n"),
            ("/app/b.js", "export const b = 1;\n"),
        ]);
        let analysis = analyze(Path::new("/app/main.mjs"), &fs).unwrap();
        assert_eq!(analysis.files.len(), 3);
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
    fn warns_about_dynamic_requires() {
        let fs = MemoryFs::new(&[(
            "/app/main.js",
            "const name = process.argv[2];\nrequire(name);\n",
        )]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        assert_eq!(analysis.warnings.len(), 1);
        assert!(analysis.warnings[0].message.contains("non-literal"));
        assert!(analysis.warnings[0].strict);
    }

    #[test]
    fn detects_websocket_global() {
        let fs = MemoryFs::new(&[("/app/main.js", "new WebSocket('ws://127.0.0.1:1');\n")]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        let capabilities = capabilities(&analysis);
        for name in ["http", "https", "crypto", "buffer"] {
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
        ];
        for source in cases {
            let fs = MemoryFs::new(&[("/app/main.js", source)]);
            let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
            let capabilities = capabilities(&analysis);
            for name in FETCH_BUILTINS {
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
    fn fetch_pulls_the_undici_builtins_only() {
        let fs = MemoryFs::new(&[("/app/main.js", "fetch('http://127.0.0.1/');\n")]);
        let analysis = analyze(Path::new("/app/main.js"), &fs).unwrap();
        for builtin in FETCH_BUILTINS {
            assert!(
                analysis.builtins.contains_key(builtin),
                "missing {builtin}: {:?}",
                analysis.builtins
            );
        }
        // `http2` and `sqlite` stay out: undici guards both, so a fetch app must pack
        // without them (and `--without http2`/`--without sqlite` must keep working).
        for builtin in ["http2", "sqlite"] {
            assert!(
                !analysis.builtins.contains_key(builtin),
                "pulled {builtin}: {:?}",
                analysis.builtins
            );
        }
        // The sixteen builtins close over nineteen modules; the CLI adds the ambient
        // `console` and `timers` on top, so a one-line fetch app has 21 capabilities.
        assert_eq!(capabilities(&analysis).len(), 19);
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
