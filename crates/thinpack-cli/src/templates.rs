use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use thinpack_caps::Module;
use thinpack_pack::{Backend, profile};

use crate::options::{DEFAULT_BACKEND, Options};

/// The template binary: the CLI looks for it next to itself, and a cache entry is a copy of it.
/// Windows binaries carry an extension, POSIX ones do not.
fn runtime_binary() -> String {
    format!("thinpack-rt{}", std::env::consts::EXE_SUFFIX)
}

#[derive(Clone, Debug)]
pub(crate) struct TemplateInfo {
    pub version: String,
    pub backend: Backend,
    pub capabilities_hash: u64,
    pub capabilities: Vec<String>,
}

impl TemplateInfo {
    fn missing(&self, required: &[&'static Module]) -> Vec<&'static str> {
        required
            .iter()
            .map(|module| module.name)
            .filter(|name| !self.capabilities.iter().any(|have| have == *name))
            .collect()
    }

    fn validate(
        &self,
        binary: &Path,
        required: &[&'static Module],
        backend: Option<Backend>,
    ) -> Result<(), Box<dyn Error>> {
        if let Some(backend) = backend
            && backend != self.backend
        {
            return Err(format!(
                "{} uses the {} backend, but --backend {backend} was requested",
                binary.display(),
                self.backend
            )
            .into());
        }
        let missing = self.missing(required);
        if !missing.is_empty() {
            return Err(format!(
                "{} is missing capabilities: {}\nrebuild it with those features or pass another --template",
                binary.display(),
                missing.join(", ")
            )
            .into());
        }
        Ok(())
    }
}

pub(crate) struct ResolvedTemplate {
    pub path: PathBuf,
    pub info: TemplateInfo,
}

impl ResolvedTemplate {
    fn discover(
        path: PathBuf,
        required: &[&'static Module],
        backend: Option<Backend>,
    ) -> Option<Self> {
        let info = inspect(&path).ok()?;
        if info.version != env!("CARGO_PKG_VERSION")
            || info.validate(&path, required, backend).is_err()
        {
            return None;
        }
        Some(Self { path, info })
    }
}

fn inspect(binary: &Path) -> Result<TemplateInfo, Box<dyn Error>> {
    let info = run(binary, "--template-info")?;
    let mut version = None;
    let mut backend = None;
    let mut template_api = None;
    let mut capabilities_hash = None;
    for line in info.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "version" => version = Some(value.to_owned()),
            "backend" => {
                backend = Some(Backend::parse(value).ok_or_else(|| {
                    format!("{} reports unknown backend `{value}`", binary.display())
                })?);
            }
            "template-api" => template_api = Some(value),
            "capabilities-hash" => {
                capabilities_hash = Some(
                    u64::from_str_radix(value.trim_start_matches("0x"), 16)
                        .map_err(|error| format!("template capabilities-hash: {error}"))?,
                );
            }
            _ => {}
        }
    }

    if template_api != Some(thinpack_pack::TEMPLATE_API) {
        return Err(format!(
            "{} reports template API {}; expected {}. Use a matching release or rebuild the template.",
            binary.display(),
            template_api.unwrap_or("(unreported)"),
            thinpack_pack::TEMPLATE_API,
        )
        .into());
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
            .join(info.backend.name())
            .join(format!("{:016x}", info.capabilities_hash))
    }

    pub(crate) fn install(
        &self,
        binary: &Path,
        info: &TemplateInfo,
    ) -> Result<PathBuf, Box<dyn Error>> {
        let directory = self.entry_dir(info);
        fs::create_dir_all(&directory)?;
        let installed = directory.join(runtime_binary());
        fs::copy(binary, &installed)?;
        fs::set_permissions(&installed, fs::metadata(binary)?.permissions())?;
        fs::write(
            directory.join("capabilities"),
            info.capabilities.join("\n") + "\n",
        )?;
        Ok(installed)
    }
    fn find(
        &self,
        required: &[&'static Module],
        backend: Option<Backend>,
    ) -> Result<Option<ResolvedTemplate>, Box<dyn Error>> {
        let mut candidates = BTreeMap::<_, Vec<_>>::new();
        for entry in self.walk()? {
            // The cache keeps entries from older CLI versions, whose templates
            // predate the current manifest format and cannot serve this pack.
            if entry.version != env!("CARGO_PKG_VERSION") {
                continue;
            }
            if backend.is_some_and(|wanted| wanted != entry.backend) {
                continue;
            }
            let binary = entry.directory.join(runtime_binary());
            if !binary.is_file() {
                continue;
            }
            if required
                .iter()
                .all(|module| entry.capabilities.contains(module.name))
            {
                candidates
                    .entry(entry.backend)
                    .or_default()
                    .push((entry.capabilities.len(), binary));
            }
        }

        let mut matches: Vec<_> = candidates
            .into_iter()
            .filter_map(|(backend, mut binaries)| {
                binaries.sort_unstable();
                binaries.into_iter().find_map(|(_, binary)| {
                    ResolvedTemplate::discover(binary, required, Some(backend))
                })
            })
            .collect();
        if matches.len() > 1 {
            return Err(format!(
                "templates for multiple backends are cached ({}); pass --backend",
                matches
                    .iter()
                    .map(|template| template.info.backend.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into());
        }
        Ok(matches.pop())
    }

    pub(crate) fn entries(&self) -> Result<Vec<CacheEntry>, Box<dyn Error>> {
        let mut entries: Vec<CacheEntry> = self
            .walk()?
            .into_iter()
            .map(|entry| CacheEntry {
                version: entry.version,
                backend: entry.backend,
                capabilities: entry.capabilities.len(),
                path: entry.directory.join(runtime_binary()),
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
                let Some(backend_kind) = Backend::parse(&backend_name) else {
                    continue;
                };
                for entry in fs::read_dir(backend.path())?.flatten() {
                    let Ok(capabilities) = fs::read_to_string(entry.path().join("capabilities"))
                    else {
                        continue;
                    };
                    found.push(CachedTemplate {
                        version: version_name.clone(),
                        backend: backend_kind,
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
    backend: Backend,
    directory: PathBuf,
    capabilities: BTreeSet<String>,
}

impl Cache {
    fn rebuild(
        &self,
        required: &[&'static Module],
        backend: Backend,
        source: Option<&Path>,
    ) -> Result<ResolvedTemplate, Box<dyn Error>> {
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

        let mut features = vec![backend.feature().to_owned()];
        for module in required {
            features.push(module.feature.to_owned());
        }
        features.sort();
        features.dedup();

        let target_dir = source.join("target/templates");
        let cargo = env::var_os("THINPACK_CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(&cargo)
            .current_dir(&source)
            .env(
                "RUSTFLAGS",
                profile::append(env::var("RUSTFLAGS").ok(), profile::rust_flags()),
            )
            // MSVC drives cl.exe, which knows none of these.
            .env(
                "CFLAGS",
                if cfg!(target_env = "msvc") {
                    env::var("CFLAGS").unwrap_or_default()
                } else {
                    profile::append(env::var("CFLAGS").ok(), profile::C_FLAGS)
                },
            )
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

        let binary = target_dir.join("release").join(runtime_binary());
        let info = inspect(&binary)?;
        if info.version != env!("CARGO_PKG_VERSION") {
            return Err(format!(
                "{} built template version {}; use a checkout for thinpack {}",
                source.display(),
                info.version,
                env!("CARGO_PKG_VERSION")
            )
            .into());
        }
        info.validate(&binary, required, Some(backend))?;
        let path = self.install(&binary, &info)?;
        Ok(ResolvedTemplate { path, info })
    }
}

pub(crate) struct CacheEntry {
    pub version: String,
    pub backend: Backend,
    pub capabilities: usize,
    pub path: PathBuf,
}

fn sibling_template() -> Option<PathBuf> {
    let template = std::env::current_exe()
        .ok()?
        .parent()?
        .join(runtime_binary());
    template.is_file().then_some(template)
}
pub(crate) fn resolve_template(
    options: &Options,
    required: &[&'static Module],
) -> Result<ResolvedTemplate, Box<dyn Error>> {
    if options.template.is_some() && options.rebuild {
        return Err("--template and --rebuild are mutually exclusive".into());
    }

    if let Some(template) = &options.template {
        let info = inspect(template)?;
        info.validate(template, required, options.backend)?;
        return Ok(ResolvedTemplate {
            path: template.clone(),
            info,
        });
    }

    let cache = Cache::from_env()?;
    // `--rebuild` answers "this app's own closure": reusing a cached superset
    // here would silently ship a runtime larger than the app needs.
    if options.rebuild {
        let backend = options.backend.unwrap_or(DEFAULT_BACKEND);
        return cache.rebuild(required, backend, options.source.as_deref());
    }
    let sibling = sibling_template()
        .and_then(|path| ResolvedTemplate::discover(path, required, options.backend));
    let backend = options
        .backend
        .or_else(|| sibling.as_ref().map(|template| template.info.backend));
    if let Some(found) = cache.find(required, backend)? {
        return Ok(found);
    }
    if let Some(sibling) = sibling {
        return Ok(sibling);
    }

    let backend = options.backend.unwrap_or(DEFAULT_BACKEND);
    Err(format!(
        "no {backend} template covers these capabilities: {}\n\
         install a matching release, pass --template <thinpack-rt>, or use --rebuild with a source checkout",
        required
            .iter()
            .map(|module| module.name)
            .collect::<Vec<_>>()
            .join(", ")
    )
    .into())
}
