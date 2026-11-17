use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::options::{DEFAULT_BACKEND, Options};

#[derive(Clone, Debug)]
pub(crate) struct TemplateInfo {
    pub version: String,
    pub backend: String,
    pub capabilities_hash: u64,
    pub capabilities: Vec<String>,
}

impl TemplateInfo {
    pub(crate) fn missing<'a>(&self, required: &'a [String]) -> Vec<&'a str> {
        required
            .iter()
            .map(String::as_str)
            .filter(|name| !self.capabilities.iter().any(|have| have == *name))
            .collect()
    }
}

pub(crate) fn inspect(binary: &Path) -> Result<TemplateInfo, Box<dyn Error>> {
    let info = run(binary, "--template-info")?;
    let mut version = None;
    let mut backend = None;
    let mut capabilities_hash = None;
    for line in info.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "version" => version = Some(value.to_owned()),
            "backend" => backend = Some(value.to_owned()),
            "capabilities-hash" => {
                capabilities_hash = Some(
                    u64::from_str_radix(value.trim_start_matches("0x"), 16)
                        .map_err(|error| format!("template capabilities-hash: {error}"))?,
                );
            }
            _ => {}
        }
    }

    let capabilities = run(binary, "--capabilities")?
        .lines()
        .filter_map(|line| line.strip_prefix("module "))
        .map(str::to_owned)
        .collect();

    Ok(TemplateInfo {
        version: version.ok_or("template reported no version")?,
        backend: backend.ok_or("template reported no backend")?,
        capabilities_hash: capabilities_hash.ok_or("template reported no capabilities hash")?,
        capabilities,
    })
}

fn run(binary: &Path, argument: &str) -> Result<String, Box<dyn Error>> {
    let output = Command::new(binary)
        .arg(argument)
        .output()
        .map_err(|error| format!("{}: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} {argument} failed: {}",
            binary.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("{} {argument}: output is not UTF-8", binary.display()).into())
}

pub(crate) struct Cache {
    root: PathBuf,
}

impl Cache {
    pub(crate) fn from_env() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            root: thinpack_pack::cache_root()?,
        })
    }

    fn entry_dir(&self, info: &TemplateInfo) -> PathBuf {
        self.root
            .join("templates")
            .join(&info.version)
            .join(&info.backend)
            .join(format!("{:016x}", info.capabilities_hash))
    }

    pub(crate) fn install(
        &self,
        binary: &Path,
        info: &TemplateInfo,
    ) -> Result<PathBuf, Box<dyn Error>> {
        let directory = self.entry_dir(info);
        fs::create_dir_all(&directory)?;
        let installed = directory.join("thinpack-rt");
        fs::copy(binary, &installed)?;
        fs::set_permissions(&installed, fs::metadata(binary)?.permissions())?;
        fs::write(
            directory.join("capabilities"),
            info.capabilities.join("\n") + "\n",
        )?;
        Ok(installed)
    }
    pub(crate) fn find(
        &self,
        required: &[String],
        backend: Option<&str>,
    ) -> Result<Option<PathBuf>, Box<dyn Error>> {
        let mut candidates = Vec::new();
        let mut backends = BTreeSet::new();
        for entry in self.walk()? {
            if backend.is_some_and(|wanted| wanted != entry.backend) {
                continue;
            }
            let binary = entry.directory.join("thinpack-rt");
            if !binary.is_file() {
                continue;
            }
            if required
                .iter()
                .all(|name| entry.capabilities.contains(name.as_str()))
            {
                backends.insert(entry.backend.clone());
                candidates.push((entry.capabilities.len(), binary));
            }
        }

        if candidates.is_empty() {
            return Ok(None);
        }
        if backend.is_none() && backends.len() > 1 {
            return Err(format!(
                "templates for multiple backends are cached ({}); pass --backend",
                backends.into_iter().collect::<Vec<_>>().join(", ")
            )
            .into());
        }
        candidates.sort();
        Ok(Some(candidates.swap_remove(0).1))
    }

    pub(crate) fn entries(&self) -> Result<Vec<CacheEntry>, Box<dyn Error>> {
        let mut entries: Vec<CacheEntry> = self
            .walk()?
            .into_iter()
            .map(|entry| CacheEntry {
                version: entry.version,
                backend: entry.backend,
                capabilities: entry.capabilities.len(),
                path: entry.directory.join("thinpack-rt"),
            })
            .collect();
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    fn walk(&self) -> Result<Vec<CachedTemplate>, Box<dyn Error>> {
        let templates = self.root.join("templates");
        if !templates.is_dir() {
            return Ok(Vec::new());
        }

        let mut found = Vec::new();
        for version in fs::read_dir(&templates)?.flatten() {
            let version_name = version.file_name().to_string_lossy().into_owned();
            for backend in fs::read_dir(version.path())?.flatten() {
                let backend_name = backend.file_name().to_string_lossy().into_owned();
                for entry in fs::read_dir(backend.path())?.flatten() {
                    let Ok(capabilities) = fs::read_to_string(entry.path().join("capabilities"))
                    else {
                        continue;
                    };
                    found.push(CachedTemplate {
                        version: version_name.clone(),
                        backend: backend_name.clone(),
                        directory: entry.path(),
                        capabilities: capabilities.lines().map(str::to_owned).collect(),
                    });
                }
            }
        }
        Ok(found)
    }
}

struct CachedTemplate {
    version: String,
    backend: String,
    directory: PathBuf,
    capabilities: BTreeSet<String>,
}

impl Cache {
    pub(crate) fn rebuild(
        &self,
        required: &[String],
        backend: &str,
        source: Option<&Path>,
    ) -> Result<PathBuf, Box<dyn Error>> {
        let source = source.map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
            Path::to_path_buf,
        );
        let source = source.canonicalize().map_err(|error| {
            format!(
                "{}: {error}; pass --source <thinpack checkout> to build a template",
                source.display()
            )
        })?;
        if !source.join("crates/thinpack-rt/Cargo.toml").is_file() {
            return Err(format!(
                "{} is not a thinpack checkout; pass --source <path>",
                source.display()
            )
            .into());
        }

        let mut features = vec![format!("backend-{backend}")];
        for name in required {
            let module =
                thinpack_caps::module(name).ok_or_else(|| format!("unknown module `{name}`"))?;
            features.push(module.feature.to_owned());
        }
        features.sort();
        features.dedup();

        let target_dir = source.join("target/templates");
        let cargo = std::env::var_os("THINPACK_CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(&cargo)
            .current_dir(&source)
            .args([
                "build",
                "--locked",
                "-p",
                "thinpack-rt",
                "--release",
                "--no-default-features",
                "--features",
            ])
            .arg(features.join(","))
            .arg("--target-dir")
            .arg(&target_dir)
            .status()
            .map_err(|error| format!("cannot run `{}`: {error}", cargo.to_string_lossy()))?;
        if !status.success() {
            return Err("building the runtime template failed".into());
        }

        let binary = target_dir.join("release/thinpack-rt");
        let info = inspect(&binary)?;
        self.install(&binary, &info)
    }
}

pub(crate) struct CacheEntry {
    pub version: String,
    pub backend: String,
    pub capabilities: usize,
    pub path: PathBuf,
}

fn sibling_template() -> Option<PathBuf> {
    let template = std::env::current_exe().ok()?.parent()?.join("thinpack-rt");
    template.is_file().then_some(template)
}
pub(crate) fn resolve_template(
    options: &Options,
    required: &[String],
) -> Result<PathBuf, Box<dyn Error>> {
    if options.template.is_some() && options.rebuild {
        return Err("--template and --rebuild are mutually exclusive".into());
    }

    if let Some(template) = &options.template {
        let info = inspect(template)?;
        let missing = info.missing(required);
        if !missing.is_empty() {
            return Err(format!(
                "{} is missing capabilities: {}\nrebuild it with those features or pass another --template",
                template.display(),
                missing.join(", ")
            )
            .into());
        }
        return Ok(template.clone());
    }

    let cache = Cache::from_env()?;
    if let Some(found) = cache.find(required, options.backend.as_deref())? {
        return Ok(found);
    }
    if options.rebuild {
        let backend = options.backend.as_deref().unwrap_or(DEFAULT_BACKEND);
        return cache.rebuild(required, backend, options.source.as_deref());
    }
    if let Some(sibling) = sibling_template()
        && options.backend.as_deref().is_none_or(|backend| {
            inspect(&sibling)
                .is_ok_and(|info| info.backend == backend && info.missing(required).is_empty())
        })
    {
        return Ok(sibling);
    }

    Err(format!(
        "no cached template covers these capabilities: {}\n\
         pass --template <thinpack-rt>, or --rebuild to build one with cargo",
        required.join(", ")
    )
    .into())
}
