use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, ExportAllDeclaration, ExportFromDeclaration, Expression,
    ImportDeclaration, ImportExpression, NewExpression, TryStatement,
};

use oxc_ast_visit::{VisitJs, walk_js};
use oxc_parser::Parser;
use oxc_span::SourceType;
use thinpack_resolve::FileSystem;

mod lines;

pub use lines::{LINE_LIMIT, bound_lines};

#[derive(Debug)]
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
                return Err(Error::UnknownBuiltin {
                    specifier,
                    from: path,
                });
            }
            let resolved = match if is_import {
                thinpack_resolve::resolve_import(&specifier, &path, fs)
            } else {
                thinpack_resolve::resolve(&specifier, &path, fs)
            } {
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
            pending.push_back(resolved.path);
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
    let source_type = SourceType::from_path(path)
        .map(|source_type| source_type.with_module(true))
        .unwrap_or_default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        let messages: Vec<String> = parsed.diagnostics.iter().map(ToString::to_string).collect();
        return Err(Error::Parse {
            path: path.to_path_buf(),
            message: messages.join("; "),
        });
    }

    let mut collector = SpecifierCollector {
        path,
        specifiers: BTreeMap::new(),
        try_depth: 0,
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

struct SpecifierCollector<'a, 'w> {
    path: &'a Path,
    specifiers: BTreeMap<(String, bool), bool>,
    try_depth: usize,
    warnings: &'w mut Vec<Warning>,
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

    fn require_call(&mut self, call: &CallExpression<'_>) {
        let target = match &call.callee {
            Expression::Identifier(identifier) => match identifier.name.as_str() {
                "require" => Some("require"),
                _ => None,
            },
            Expression::StaticMemberExpression(member) => match member.object {
                Expression::Identifier(ref object) => {
                    match (object.name.as_str(), member.property.name.as_str()) {
                        ("require", "resolve") => Some("require.resolve"),
                        ("module", "require") => Some("module.require"),
                        ("process", "getBuiltinModule") => Some("process.getBuiltinModule"),
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => None,
        };
        let Some(target) = target else {
            return;
        };

        match call.arguments.first() {
            Some(Argument::StringLiteral(literal)) => {
                self.add_specifier(js_string(literal.value), false);
            }
            Some(_) => self.warnings.push(Warning {
                message: format!(
                    "{}: `{target}(...)` with a non-literal argument cannot be analyzed",
                    self.path.display()
                ),
                strict: true,
            }),
            None => {}
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
        walk_js::walk_call_expression(self, call);
    }

    fn visit_try_statement(&mut self, statement: &TryStatement<'a>) {
        self.try_depth += 1;
        walk_js::walk_try_statement(self, statement);
        self.try_depth -= 1;
    }

    fn visit_new_expression(&mut self, expression: &NewExpression<'a>) {
        let is_websocket = match &expression.callee {
            Expression::Identifier(identifier) => identifier.name == "WebSocket",
            Expression::StaticMemberExpression(member) => {
                matches!(&member.object, Expression::Identifier(object) if object.name == "globalThis")
                    && member.property.name == "WebSocket"
            }
            _ => false,
        };
        if is_websocket {
            for builtin in ["http", "https", "crypto", "buffer"] {
                self.add_specifier(builtin.to_owned(), false);
            }
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
        assert!(analysis.warnings.is_empty());
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
        let fs = MemoryFs::new(&[("/app/main.js", "const name = './x';\nrequire(name);\n")]);
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
