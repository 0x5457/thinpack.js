//! `dist`: build the artifacts users download, verify them end to end, and archive them.
//!
//! The two binaries ship side by side because that is how the CLI finds its runtime template
//! (`templates::sibling_template`); a release that separated them would download but refuse to
//! pack. Before writing the archive the command packs and runs `examples/hello.js` from the
//! staging directory with an empty template cache — the exact path a user takes after
//! unpacking — so no release ships a pair that cannot pack.
//!
//! The archive is reproducible for the same build: sorted entries, fixed uid/gid/mtime, explicit
//! modes. The binaries themselves are not portable across checkouts — `templates.rs` compiles its
//! own source directory in as the `--rebuild` default — so "same input" includes the source path.

use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::build;
use crate::capabilities;
use thinpack_pack::{Backend, profile};

const USAGE: &str =
    "Usage: cargo xtask dist [--backend <quickjs|v8|jsc>] [--target <triple>] [--out <dir>]";

/// Binaries in the staging directory. The names carry no extension; the target decides that.
const BINARIES: [&str; 2] = ["thinpack", "thinpack-rt"];

/// Documentation shipped next to the binaries.
const DOCUMENTS: [&str; 2] = ["LICENSE", "README.md"];

/// The app the staged pair has to pack and run before the archive is written.
const PROBE: &str = "examples/hello.js";

/// Output of `examples/hello.js`: a synchronous log and a microtask. Both have to appear, so a
/// runtime that starts but never drains the microtask queue fails the release.
const PROBE_EXPECTED: [&str; 2] = ["answer: 42", "microtask completed"];

/// Everything the staging directory holds, sorted by name: the programs, then the documents.
fn staged() -> Vec<(&'static str, bool)> {
    let mut entries: Vec<(&str, bool)> = BINARIES
        .map(|name| (name, true))
        .into_iter()
        .chain(DOCUMENTS.map(|name| (name, false)))
        .collect();
    entries.sort_unstable();
    entries
}

struct Config {
    backend: Backend,
    target: String,
    out: PathBuf,
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    match parse(args).and_then(|config| dist(&config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[String]) -> Result<Config, String> {
    let mut backend = Backend::QuickJs;
    let mut target = None;
    let mut out = crate::repo_root().join("dist");

    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--backend" => {
                let value = value_of(&mut args, arg)?;
                backend = Backend::parse(value).ok_or_else(|| {
                    format!("unknown backend `{value}`; expected quickjs, v8 or jsc")
                })?;
            }
            "--target" => target = Some(value_of(&mut args, arg)?.to_owned()),
            "--out" => out = PathBuf::from(value_of(&mut args, arg)?),
            _ => return Err(format!("unknown argument `{arg}`\n{USAGE}")),
        }
    }

    Ok(Config {
        backend,
        target: target.map_or_else(build::host_triple, Ok)?,
        out,
    })
}

/// `dist` runs the artifact it just built, so a JSC archive can only be produced on macOS for
/// a macOS target.
fn check_host(config: &Config) -> Result<(), String> {
    if !matches!(config.backend, Backend::Jsc) {
        return Ok(());
    }
    if !cfg!(target_os = "macos") {
        return Err("the jsc backend uses the macOS system JavaScriptCore framework".to_owned());
    }
    if !config.target.contains("apple-darwin") {
        return Err("the jsc backend is available for macOS targets only".to_owned());
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

fn dist(config: &Config) -> Result<(), String> {
    check_host(config)?;
    let table = capabilities::load()?;
    let stage = config.out.join(config.stem());

    println!("staging {}", stage.display());
    let built = build(config, &table)?;
    stage_files(config, &stage, &built)?;
    // Always `--target`, so the artifact is never published unverified: a static musl build
    // runs on the glibc host that produced it, and a target that cannot run here is a mistake
    // in the release matrix, not a reason to skip the check.
    verify(config, &stage, &table)?;
    let archive = archive(config, &stage)?;

    let bytes = fs::metadata(&archive)
        .map_err(|error| format!("{}: {error}", archive.display()))?
        .len();
    println!(
        "\n{}  {} bytes ({} MiB), sha256 {}",
        archive.display(),
        bytes,
        bytes / 1_048_576,
        build::sha256_file(&archive)?
    );
    Ok(())
}

fn build(config: &Config, table: &[capabilities::Module]) -> Result<PathBuf, String> {
    let target_dir = build::target_root().join("dist");
    let mut features = vec![format!("thinpack-rt/backend-{}", config.backend.name())];
    features.extend(
        table
            .iter()
            .map(|module| format!("thinpack-rt/{}", module.feature)),
    );

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut command = Command::new(cargo);
    command.current_dir(crate::repo_root()).args([
        "build",
        "--release",
        "--locked",
        "--package",
        "thinpack-cli",
        "--package",
        "thinpack-rt",
        "--no-default-features",
        "--features",
    ]);
    command.arg(features.join(","));
    command.arg("--target-dir").arg(&target_dir);
    command.args(["--target", &config.target]);
    // The template profile: `crates/thinpack-cli/src/templates.rs` rebuilds templates with the
    // same flags, so a downloaded pair and a locally rebuilt one agree. An empty value would
    // still shadow the per-target `rustflags` in `.cargo/config.toml`, which is where the MSVC
    // link arguments live, so it is only applied when there is something to say.
    let rustflags = profile::append(
        env::var("RUSTFLAGS").ok(),
        &profile::rust_flags_for_target(&config.target),
    );
    if !rustflags.trim().is_empty() {
        command.env("RUSTFLAGS", rustflags);
    }
    // MSVC has no gcc-style unwind flags; everything else is clang or gcc.
    if !config.target.contains("msvc") {
        command.env(
            "CFLAGS",
            profile::append(env::var("CFLAGS").ok(), profile::C_FLAGS),
        );
    }

    let status = command
        .status()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if !status.success() {
        return Err(format!("build failed ({status})"));
    }
    Ok(target_dir.join(&config.target).join("release"))
}

fn stage_files(config: &Config, stage: &Path, built: &Path) -> Result<(), String> {
    if stage.exists() {
        fs::remove_dir_all(stage).map_err(|error| format!("{}: {error}", stage.display()))?;
    }
    fs::create_dir_all(stage).map_err(|error| format!("{}: {error}", stage.display()))?;

    for (name, binary) in staged() {
        let file = config.file_name(name, binary);
        let source = if binary {
            built.join(&file)
        } else {
            crate::repo_root().join(&file)
        };
        fs::copy(&source, stage.join(&file))
            .map_err(|error| format!("{}: {error}", source.display()))?;
    }
    Ok(())
}

/// What a user does after unpacking: `thinpack pack <app>` with nothing but the sibling runtime,
/// in a cache that cannot answer for it.
fn verify(config: &Config, stage: &Path, table: &[capabilities::Module]) -> Result<(), String> {
    let runtime = stage.join(config.file_name("thinpack-rt", true));
    let info = output(Command::new(&runtime).arg("--template-info"))?;
    let expected = [
        format!("version {}", env!("CARGO_PKG_VERSION")),
        format!("backend {}", config.backend.name()),
    ];
    for expected in &expected {
        if !info.lines().any(|line| line == expected) {
            return Err(format!(
                "{} does not report `{expected}`:\n{info}",
                runtime.display()
            ));
        }
    }

    // The probe is a packed executable, so it needs the target's executable suffix:
    // Windows decides that by extension, and `thinpack pack -o hello` writes `hello`.
    // The archive claims a template carrying every built-in module; a feature lost between
    // `capabilities.toml` and the registry would otherwise pass the probe below, and the app
    // that needs it would fail at `require` time on a user's machine.
    let reported = output(Command::new(&runtime).arg("--capabilities"))?;
    for module in table {
        let expected = format!("module {}", module.name);
        if !reported.lines().any(|have| have == expected) {
            return Err(format!(
                "{} does not carry `{}`; the packaged template is not the full set",
                runtime.display(),
                module.name
            ));
        }
    }

    let scratch = build::target_root().join("dist/probe");
    let cache = scratch.join("cache");
    if cache.exists() {
        fs::remove_dir_all(&cache).map_err(|error| format!("{}: {error}", cache.display()))?;
    }
    fs::create_dir_all(&cache).map_err(|error| format!("{}: {error}", cache.display()))?;

    let artifact = scratch.join(config.file_name("hello", true));
    output(
        Command::new(stage.join(config.file_name("thinpack", true)))
            .current_dir(crate::repo_root())
            .env("THINPACK_CACHE", &cache)
            .args(["pack", PROBE, "--backend", config.backend.name(), "-o"])
            .arg(&artifact),
    )?;

    let stdout = output(&mut Command::new(&artifact))?;
    for expected in PROBE_EXPECTED {
        if !stdout.contains(expected) {
            return Err(format!(
                "{} did not print `{expected}`:\n{stdout}",
                artifact.display()
            ));
        }
    }
    println!("verified: the staged pair packs and runs {PROBE}");
    Ok(())
}

fn archive(config: &Config, stage: &Path) -> Result<PathBuf, String> {
    let stem = config.stem();
    let path = config.out.join(format!("{stem}.tar.gz"));
    fs::create_dir_all(&config.out)
        .map_err(|error| format!("{}: {error}", config.out.display()))?;

    let encoder = GzEncoder::new(
        File::create(&path).map_err(|error| format!("{}: {error}", path.display()))?,
        Compression::best(),
    );
    let mut tar = tar::Builder::new(encoder);

    for (name, binary) in staged() {
        let file_name = config.file_name(name, binary);
        let file = stage.join(&file_name);
        let bytes = fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(bytes.len() as u64);
        header.set_mode(if binary { 0o755 } else { 0o644 });
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_cksum();
        tar.append_data(&mut header, format!("{stem}/{file_name}"), &bytes[..])
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    tar.into_inner()
        .and_then(GzEncoder::finish)
        .map_err(|error| format!("{}: {error}", path.display()))?;

    let sums = config.out.join(format!("{stem}.tar.gz.sha256"));
    fs::write(
        &sums,
        format!("{}  {stem}.tar.gz\n", build::sha256_file(&path)?),
    )
    .map_err(|error| format!("{}: {error}", sums.display()))?;
    Ok(path)
}

impl Config {
    /// The same name for the staging directory, archive stem and archive contents.
    fn stem(&self) -> String {
        let suffix = self.backend.archive_suffix();
        format!(
            "thinpack-{}{suffix}-{}",
            env!("CARGO_PKG_VERSION"),
            self.target
        )
    }

    fn exe_suffix(&self) -> &'static str {
        if self.target.contains("windows") {
            ".exe"
        } else {
            ""
        }
    }

    /// A staged file. Programs take the target's executable suffix; documentation keeps its name.
    fn file_name(&self, name: &str, binary: bool) -> String {
        if binary {
            format!("{name}{}", self.exe_suffix())
        } else {
            name.to_owned()
        }
    }
}

fn output(command: &mut Command) -> Result<String, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{program}: output is not UTF-8"))
}
