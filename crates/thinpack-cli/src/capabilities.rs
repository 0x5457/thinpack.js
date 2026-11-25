use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};

use thinpack_analyze::{Analysis, StaticDir};
use thinpack_caps::Module;

use thinpack_resolve::normalize;

use crate::bundle::project_root;
use crate::options::Options;

pub(crate) struct Analyzed {
    pub entry: PathBuf,
    pub root: PathBuf,
    pub analysis: Analysis,
    pub capabilities: Vec<&'static Module>,
}

pub(crate) fn analyze_entry(options: &Options, pack: bool) -> Result<Analyzed, Box<dyn Error>> {
    // `absolute` keeps `..` components; a manifest record built from them would
    // never match the runtime's normalized lookup, so the whole pipeline works
    // on lexically clean paths.
    let entry = normalize(&std::path::absolute(
        options.entry.as_ref().expect("entry checked"),
    )?);
    let root = project_root(&entry)?;
    let allow_missing: BTreeSet<String> = options.allow_missing.iter().cloned().collect();
    let mut conditions = options.conditions.clone();
    conditions.sort_unstable();
    conditions.dedup();
    let included = expand_includes(&root, &options.include)?;
    let mut analysis = analyze_graph(&entry, &root, &included, &allow_missing, &conditions)?;
    if pack {
        // A module outside the root cannot be reached from the payload. Pack leaves
        // optional ones out; the warning belongs with the other optional warnings so
        // `--strict` and `--list-capabilities` treat it like the rest.
        for path in &analysis.files {
            if !path.starts_with(&root) && !analysis.required.contains(path) {
                analysis.warnings.push(thinpack_analyze::Warning {
                    message: format!(
                        "{}: optional module resolves outside the project root {}; it is left out of the payload and the app must handle its absence",
                        path.display(),
                        root.display()
                    ),
                    strict: false,
                });
            }
        }
    }
    for warning in &analysis.warnings {
        if options.strict && warning.strict {
            return Err(format!(
                "{}\n(--strict turns this warning into an error)",
                warning.message
            )
            .into());
        }
        eprintln!("warning: {}", warning.message);
    }
    let capabilities = apply_overrides(&analysis, options)?;
    Ok(Analyzed {
        entry,
        root,
        analysis,
        capabilities,
    })
}

/// Analyze the entry and explicit include roots, expanding every
/// package-relative directory the graph enumerates with `fs.readdir` or
/// `fs.readdirSync` as if it had been passed to `--include`. A directory can
/// contain a module that scans another directory, so repeat until no new
/// directory appears.
fn analyze_graph(
    entry: &Path,
    root: &Path,
    includes: &[PathBuf],
    allow_missing: &BTreeSet<String>,
    conditions: &[String],
) -> Result<Analysis, Box<dyn Error>> {
    let mut includes = includes.to_vec();
    let mut expanded = BTreeSet::new();
    let mut skipped = BTreeMap::new();
    loop {
        let mut analysis = thinpack_analyze::analyze_roots_with(
            entry,
            &includes,
            &thinpack_resolve::HostFs,
            allow_missing,
            conditions,
        )?;
        let mut discovered = Vec::new();
        for (directory, filter) in &analysis.static_dirs {
            let directory = normalize(directory);
            if !expanded.insert((directory.clone(), filter.clone())) {
                continue;
            }
            // The payload is rooted at the project, so a directory above it cannot
            // be reached from `/app`. Leave it to the host.
            if !directory.starts_with(root) {
                skipped.insert(directory, "is outside the project root");
                continue;
            }
            if !expandable_scan(root, &directory) {
                skipped.insert(
                    directory,
                    "is the project root, a package root, node_modules, or a symlink",
                );
                continue;
            }
            collect_files(&directory, false, Some(filter), &mut discovered)?;
        }
        if discovered.is_empty() {
            for (directory, reason) in skipped {
                analysis.warnings.push(thinpack_analyze::Warning {
                    message: format!(
                        "static directory {} {reason}; add it with --include",
                        directory.display()
                    ),
                    strict: false,
                });
            }
            analysis.warnings.sort();
            return Ok(analysis);
        }
        includes.extend(discovered);
        includes.sort();
        includes.dedup();
    }
}

/// Whether the graph's scan of `directory` can stand in for `--include`. The
/// directory is already known to be inside `root`; it also has to stay a bounded
/// part of the project instead of sweeping the project root, a package root,
/// `node_modules`, or a symlinked copy of one of those.
fn expandable_scan(root: &Path, directory: &Path) -> bool {
    let Ok(relative) = directory.strip_prefix(root) else {
        return false;
    };
    if relative.as_os_str().is_empty() || relative.ends_with("node_modules") {
        return false;
    }
    let mut path = root.to_path_buf();
    for part in relative {
        path.push(part);
        if !std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
            return false;
        }
    }
    !directory.join("package.json").is_file()
}

fn expand_includes(root: &Path, includes: &[PathBuf]) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let root = normalize(root);
    let mut files = Vec::new();
    for include in includes {
        let joined = if include.is_absolute() {
            include.clone()
        } else {
            root.join(include)
        };
        let path = normalize(&joined);
        // A packed executable only sees the virtual `/app` tree, so an include
        // outside the project root would ship as unreachable weight.
        if !path.starts_with(&root) {
            return Err(format!(
                "--include {} resolves to {}, outside the project root {}; \
                 copy the file into the project or pack it with --asset",
                include.display(),
                path.display(),
                root.display()
            )
            .into());
        }
        let metadata = std::fs::metadata(&path).map_err(|error| {
            format!(
                "{}: {error}\n--include paths are relative to the project root {}",
                path.display(),
                root.display()
            )
        })?;
        if metadata.is_dir() {
            collect_files(&path, true, None, &mut files)?;
        } else {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Collect every regular file under `directory`. An `--include` follows links; an
/// implicitly scanned directory does not, so a link cannot carry host files into
/// the payload.
fn collect_files(
    directory: &Path,
    follow_links: bool,
    filter: Option<&StaticDir>,
    files: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = if follow_links {
            entry.metadata()?.file_type()
        } else {
            entry.file_type()?
        };
        if file_type.is_dir() {
            if entry.file_name() != ".git" && (follow_links || entry.file_name() != "node_modules")
            {
                collect_files(&path, follow_links, filter, files)?;
            }
        } else if file_type.is_file() && filter.is_none_or(|filter| filter.accepts(&path)) {
            files.push(path);
        }
    }
    Ok(())
}

fn apply_overrides(
    analysis: &Analysis,
    options: &Options,
) -> Result<Vec<&'static Module>, Box<dyn Error>> {
    let mut roots: BTreeSet<String> = analysis
        .builtins
        .keys()
        .filter_map(|builtin| thinpack_caps::module_for_builtin(builtin))
        .map(|module| module.name.to_owned())
        .collect();
    roots.extend(thinpack_caps::baseline().map(|module| module.name.to_owned()));

    for module_name in &options.without {
        let module =
            thinpack_caps::module(module_name).ok_or_else(|| unknown_module(module_name))?;
        if module.ambient {
            return Err(format!("cannot remove ambient module `{module_name}`").into());
        }
        for builtin in module.builtins {
            if let Some(files) = analysis.builtins.get(*builtin) {
                let files: Vec<String> = files
                    .iter()
                    .map(|file| file.display().to_string())
                    .collect();
                return Err(format!(
                    "cannot remove `{module_name}`: `{builtin}` is required by {}",
                    files.join(", ")
                )
                .into());
            }
        }
        let dependents: Vec<&str> = thinpack_caps::MODULES
            .iter()
            .filter(|candidate| {
                candidate.deps.contains(&module_name.as_str()) && roots.contains(candidate.name)
            })
            .map(|candidate| candidate.name)
            .collect();
        if !dependents.is_empty() {
            return Err(format!(
                "cannot remove `{module_name}`: required by {}",
                dependents.join(", ")
            )
            .into());
        }
        if !roots.contains(module_name) {
            return Err(format!("module `{module_name}` is not part of this build").into());
        }
        roots.remove(module_name);
    }

    for module_name in &options.with {
        if thinpack_caps::module(module_name).is_none() {
            return Err(unknown_module(module_name).into());
        }
        roots.insert(module_name.clone());
    }

    let roots: Vec<String> = roots.into_iter().collect();
    Ok(thinpack_caps::closure(&roots)?)
}

fn unknown_module(name: &str) -> String {
    format!(
        "unknown module `{name}`\nRun `thinpack capabilities` to see the modules supported by this CLI."
    )
}

pub(crate) fn print_capabilities(
    analysis: &Analysis,
    capabilities: &[&'static Module],
    root: &Path,
) {
    println!("{:<12} {:<28} required by", "module", "builtins");
    for module in capabilities {
        let mut files = BTreeSet::new();
        for builtin in module.builtins {
            if let Some(required_by) = analysis.builtins.get(*builtin) {
                files.extend(required_by);
            }
        }
        let required_by: Vec<String> = files
            .iter()
            .map(|file| {
                file.strip_prefix(root)
                    .unwrap_or(file)
                    .display()
                    .to_string()
            })
            .collect();
        println!(
            "{:<12} {:<28} {}",
            module.name,
            module.builtins.join(", "),
            if required_by.is_empty() {
                if module.ambient {
                    "(ambient)".to_owned()
                } else {
                    "-".to_owned()
                }
            } else {
                required_by.join(", ")
            }
        );
    }
}
