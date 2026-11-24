use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{fmt::Write as _, fs};

use thinpack_pack::profile;

use crate::capabilities;
use crate::measure::gzip_size;

const MEASUREMENTS_DIR: &str = "measurements";
const BINARY_NAME: &str = "thinpack-rt";

pub(crate) use thinpack_pack::Backend;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Profile {
    Release,
    Small,
}

#[derive(Clone, PartialEq, Eq)]
enum Modules {
    All,
    Some(Vec<String>),
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    match parse(args).and_then(build_all) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

impl Modules {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "all" => Ok(Self::All),
            "core" => Ok(Self::Some(Vec::new())),
            list => {
                let mut names: Vec<String> =
                    list.split(',').map(|name| name.trim().to_owned()).collect();
                if names.iter().any(String::is_empty) {
                    return Err(format!("module list `{value}` contains an empty entry"));
                }
                names.sort();
                names.dedup();
                Ok(Self::Some(names))
            }
        }
    }

    fn name(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Some(names) if names.is_empty() => "core".to_owned(),
            Self::Some(names) => names.join("+"),
        }
    }
    fn features(&self, table: &[capabilities::Module]) -> Result<Vec<String>, String> {
        match self {
            Self::All => Ok(table.iter().map(|module| module.feature.clone()).collect()),
            Self::Some(names) => names
                .iter()
                .map(|name| {
                    table
                        .iter()
                        .find(|module| &module.name == name)
                        .map(|module| module.feature.clone())
                        .ok_or_else(|| format!("no module `{name}` in the capability table"))
                })
                .collect(),
        }
    }
}

impl Profile {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "release" => Some(Self::Release),
            "small" => Some(Self::Small),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Small => "small",
        }
    }
    /// The profile's directory below Cargo's target root.
    fn target_dir(self, backend: Backend, modules: &Modules) -> PathBuf {
        let backend_dir = match backend {
            Backend::QuickJs => "",
            Backend::V8 => "v8",
            Backend::Jsc => "jsc",
        };
        let dir = match self {
            Self::Release => backend_dir.to_owned(),
            Self::Small if backend_dir.is_empty() => "small".to_owned(),
            Self::Small => format!("{backend_dir}/small"),
        };
        match modules {
            Modules::All => PathBuf::from(dir),
            Modules::Some(_) if dir.is_empty() => PathBuf::from(modules.name()),
            Modules::Some(_) => PathBuf::from(format!("{dir}-{}", modules.name())),
        }
    }
}

fn parse(args: &[String]) -> Result<(Backend, Vec<Profile>, Modules), String> {
    let mut backend = Backend::QuickJs;
    let mut profiles = vec![Profile::Release, Profile::Small];
    let mut modules = Modules::All;

    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--backend" => {
                let value = value_of(&mut args, arg)?;
                backend = Backend::parse(value).ok_or_else(|| {
                    format!("unknown backend `{value}`; expected quickjs, v8 or jsc")
                })?;
            }
            "--profile" => {
                let value = value_of(&mut args, arg)?;
                profiles = match value {
                    "all" => vec![Profile::Release, Profile::Small],
                    _ => vec![Profile::parse(value).ok_or_else(|| {
                        format!("unknown profile `{value}`; expected release, small or all")
                    })?],
                };
            }
            "--modules" => {
                let value = value_of(&mut args, arg)?;
                modules = Modules::parse(value)?;
            }
            _ => return Err(format!("unknown argument `{arg}`")),
        }
    }

    Ok((backend, profiles, modules))
}

/// `size` measures the host build: JSC is the macOS system framework and has no other platform.
fn check_host(backend: Backend) -> Result<(), String> {
    if matches!(backend, Backend::Jsc) && !cfg!(target_os = "macos") {
        return Err("the jsc backend uses the macOS system JavaScriptCore framework".to_owned());
    }
    Ok(())
}

fn value_of<'a>(
    args: &mut impl Iterator<Item = &'a String>,
    option: &str,
) -> Result<&'a str, String> {
    args.next()
        .map(String::as_str)
        .ok_or_else(|| format!("`{option}` requires a value"))
}

fn build_all((backend, profiles, modules): (Backend, Vec<Profile>, Modules)) -> Result<(), String> {
    check_host(backend)?;
    let table = capabilities::load()?;
    let features = modules.features(&table)?;
    for profile in profiles {
        measure(backend, profile, &modules, &features)?;
    }
    Ok(())
}

fn measure(
    backend: Backend,
    profile: Profile,
    modules: &Modules,
    features: &[String],
) -> Result<(), String> {
    let binary = build(backend, profile, modules, features)?;
    let absolute = crate::repo_root().join(&binary);
    let bytes = fs::metadata(&absolute)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("{}: {error}", binary.display()))?;
    let gzip_bytes = gzip_size(&absolute)?;
    let sha256 = sha256(&absolute)?;
    let (rustc, target) = rustc_identity()?;
    let date = today()?;

    let record = Record {
        binary: binary.display().to_string(),
        bytes,
        gzip_bytes,
        sha256,
        target,
        rustc,
        backend: backend.name(),
        profile: profile.name(),
        modules: modules.name(),
        date,
    };

    let directory = crate::repo_root().join(MEASUREMENTS_DIR);
    fs::create_dir_all(&directory).map_err(|error| format!("{MEASUREMENTS_DIR}: {error}"))?;
    let path = directory.join(record.file_name());
    fs::write(&path, record.to_json()).map_err(|error| format!("{}: {error}", path.display()))?;

    println!(
        "{:<8}{:<8}{:<6}{:>10} bytes  gzip {:>9}  ->  {}",
        record.backend,
        record.profile,
        record.modules,
        record.bytes,
        record.gzip_bytes,
        path.strip_prefix(crate::repo_root())
            .unwrap_or(&path)
            .display()
    );
    Ok(())
}

fn build(
    backend: Backend,
    profile: Profile,
    modules: &Modules,
    module_features: &[String],
) -> Result<PathBuf, String> {
    let target_dir = profile.target_dir(backend, modules);
    let mut features = vec![backend.feature().to_owned()];
    features.extend(module_features.iter().cloned());

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut command = Command::new(cargo);
    command.args([
        "build",
        "--release",
        "--locked",
        "--package",
        "thinpack-rt",
        "--bin",
        BINARY_NAME,
        "--no-default-features",
        "--features",
    ]);
    command.arg(features.join(","));
    let target_dir = crate::build::target_root().join(&target_dir);
    command.arg("--target-dir").arg(&target_dir);

    if profile == Profile::Small {
        command.env(
            "RUSTFLAGS",
            profile::append(env::var("RUSTFLAGS").ok(), profile::rust_flags()),
        );
        command.env(
            "CFLAGS",
            profile::append(env::var("CFLAGS").ok(), profile::C_FLAGS),
        );
    }

    let status = command
        .status()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if !status.success() {
        return Err(format!(
            "build failed ({status}) for backend {}",
            backend.name()
        ));
    }

    Ok(target_dir.join("release").join(BINARY_NAME))
}

struct Record {
    binary: String,
    target: String,
    rustc: String,
    backend: &'static str,
    profile: &'static str,
    modules: String,
    date: String,
    bytes: u64,
    gzip_bytes: u64,
    sha256: String,
}

impl Record {
    fn file_name(&self) -> String {
        format!(
            "{}-{}-{}-{}.json",
            self.target, self.backend, self.profile, self.modules
        )
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "measured binaries stay far below 2^53 bytes, where `u64` → `f64` is exact"
    )]
    fn to_json(&self) -> String {
        let mut json = String::new();
        let _ = write!(
            json,
            concat!(
                "{{\n",
                "  \"binary\": \"{}\",\n",
                "  \"backend\": \"{}\",\n",
                "  \"profile\": \"{}\",\n",
                "  \"modules\": \"{}\",\n",
                "  \"target\": \"{}\",\n",
                "  \"rustc\": \"{}\",\n",
                "  \"date\": \"{}\",\n",
                "  \"bytes\": {},\n",
                "  \"MB\": {:.4},\n",
                "  \"MiB\": {:.4},\n",
                "  \"gzip_bytes\": {},\n",
                "  \"sha256\": \"{}\"\n",
                "}}\n"
            ),
            self.binary,
            self.backend,
            self.profile,
            self.modules,
            self.target,
            self.rustc,
            self.date,
            self.bytes,
            self.bytes as f64 / 1_000_000.0,
            self.bytes as f64 / 1_048_576.0,
            self.gzip_bytes,
            self.sha256,
        );
        json
    }
}

fn sha256(binary: &Path) -> Result<String, String> {
    crate::build::sha256_file(binary)
}

fn rustc_identity() -> Result<(String, String), String> {
    let output = Command::new("rustc")
        .args(["-vV"])
        .output()
        .map_err(|error| format!("rustc: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text
        .lines()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| "rustc -vV produced no output".to_owned())?;
    let host = text
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "rustc -vV output has no host line".to_owned())?;
    Ok((version, host))
}

fn today() -> Result<String, String> {
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output()
        .map_err(|error| format!("date: {error}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
