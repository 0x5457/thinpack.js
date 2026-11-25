use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};

use thinpack_analyze::Analysis;
use thinpack_caps::Module;

use thinpack_resolve::{is_javascript, normalize};

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
    let mut analysis = thinpack_analyze::analyze_with(
        &entry,
        &thinpack_resolve::HostFs,
        &allow_missing,
        &conditions,
    )?;
    let included = expand_includes(&root, &options.include)?;
    for file in &included {
        if is_javascript(file) {
            analysis.merge(thinpack_analyze::analyze_with(
                file,
                &thinpack_resolve::HostFs,
                &allow_missing,
                &conditions,
            )?);
        } else {
            if !analysis.files.contains(file) {
                analysis.files.push(file.clone());
            }
            // An explicit include is required even if the graph reached the file
            // through an optional edge.
            analysis.required.insert(file.clone());
        }
    }
    analysis.files.sort();
    analysis.files.dedup();
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
            collect_files(&path, &mut files)?;
        } else {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            if entry.file_name() != ".git" {
                collect_files(&path, files)?;
            }
        } else if metadata.is_file() {
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
