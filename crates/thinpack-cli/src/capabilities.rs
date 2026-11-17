use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};

use thinpack_analyze::Analysis;

use crate::bundle::project_root;
use crate::options::Options;

pub(crate) struct Analyzed {
    pub entry: PathBuf,
    pub root: PathBuf,
    pub analysis: Analysis,
    pub capabilities: Vec<String>,
}

pub(crate) fn analyze_entry(options: &Options) -> Result<Analyzed, Box<dyn Error>> {
    let entry = std::path::absolute(options.entry.as_ref().expect("entry checked"))?;
    let root = project_root(&entry)?;
    let mut analysis = thinpack_analyze::analyze(&entry, &thinpack_resolve::HostFs)?;
    let included = expand_includes(&root, &options.include)?;
    for file in &included {
        if is_javascript(file) {
            analysis.merge(thinpack_analyze::analyze(file, &thinpack_resolve::HostFs)?);
        } else if !analysis.files.contains(file) {
            analysis.files.push(file.clone());
        }
    }
    analysis.files.sort();
    analysis.files.dedup();
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

fn is_javascript(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "cjs" | "mjs")
    )
}

fn expand_includes(root: &Path, includes: &[PathBuf]) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    for include in includes {
        let path = if include.is_absolute() {
            include.clone()
        } else {
            root.join(include)
        };
        let metadata =
            std::fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
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

fn apply_overrides(analysis: &Analysis, options: &Options) -> Result<Vec<String>, Box<dyn Error>> {
    let mut roots: BTreeSet<String> = analysis
        .builtins
        .keys()
        .filter_map(|builtin| thinpack_caps::module_for_builtin(builtin))
        .map(|module| module.name.to_owned())
        .collect();
    roots.extend(thinpack_caps::baseline().map(|module| module.name.to_owned()));

    for module_name in &options.without {
        let module = thinpack_caps::module(module_name)
            .ok_or_else(|| format!("unknown module `{module_name}`"))?;
        if module.ambient {
            return Err(format!("cannot remove ambient module `{module_name}`").into());
        }
        if !roots.contains(module_name) {
            return Err(format!("module `{module_name}` is not part of this build").into());
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
        roots.remove(module_name);
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
    }

    for module_name in &options.with {
        if thinpack_caps::module(module_name).is_none() {
            return Err(format!("unknown module `{module_name}`").into());
        }
        roots.insert(module_name.clone());
    }

    let roots: Vec<String> = roots.into_iter().collect();
    Ok(thinpack_caps::closure(&roots)?
        .iter()
        .map(|module| module.name.to_owned())
        .collect())
}

pub(crate) fn print_capabilities(analysis: &Analysis, capabilities: &[String], root: &Path) {
    println!("{:<12} {:<28} required by", "module", "builtins");
    for name in capabilities {
        let Some(module) = thinpack_caps::module(name) else {
            continue;
        };
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
            name,
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
