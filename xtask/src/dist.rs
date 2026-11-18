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
use std::fmt::Write as _;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};

use crate::capabilities;
use crate::size::Backend;

const USAGE: &str =
    "Usage: cargo xtask dist [--backend <quickjs|v8>] [--target <triple>] [--out <dir>]";

/// Binaries in the staging directory. The names carry no extension; the target decides that.
const BINARIES: [&str; 2] = ["thinpack", "thinpack-rt"];

/// Documentation shipped next to the binaries.
const DOCUMENTS: [&str; 2] = ["LICENSE", "README.md"];

/// The app the staged pair has to pack and run before the archive is written.
const PROBE: &str = "examples/hello.js";

/// Output of `examples/hello.js`: a synchronous log and a microtask. Both have to appear, so a
/// runtime that starts but never drains the microtask queue fails the release.
const PROBE_EXPECTED: [&str; 2] = ["answer: 42", "microtask completed"];

// `-z pack-relative-relocs` packs every RELATIVE relocation into a `DT_RELR` block: 553 KB
// smaller on this runtime, and the loader has to understand `DT_RELR` to start the binary
// (glibc 2.36). Only x86_64 GNU ld implements the flag; elsewhere it is ignored with a warning,
// which the release build treats as an error.
const PACK_RELATIVE_RELOCS: &str = "-C link-arg=-Wl,-z,pack-relative-relocs";
const UNWIND: &str = "-C force-unwind-tables=no";
const C_FLAGS: &str = "-fno-asynchronous-unwind-tables -fno-unwind-tables";

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
    /// A build for the host needs no `--target`, so its output sits in the flat directory.
    host: bool,
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
                backend = Backend::parse(value)
                    .ok_or_else(|| format!("unknown backend `{value}`; expected quickjs or v8"))?;
            }
            "--target" => target = Some(value_of(&mut args, arg)?.to_owned()),
            "--out" => out = PathBuf::from(value_of(&mut args, arg)?),
            _ => return Err(format!("unknown argument `{arg}`\n{USAGE}")),
        }
    }

    let host = host_triple();
    Ok(Config {
        backend,
        host: target.as_deref().is_none_or(|target| target == host),
        target: target.unwrap_or(host),
        out,
    })
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
    let table = capabilities::load()?;
    let stage = config.out.join(config.stem());

    println!("staging {}", stage.display());
    let built = build(config, &table)?;
    stage_files(config, &stage, &built)?;
    if config.host {
        verify(config, &stage, &table)?;
    } else {
        println!(
            "skipping the probe: {} cannot run on this host",
            config.target
        );
    }
    let archive = archive(config, &stage)?;

    let bytes = fs::metadata(&archive)
        .map_err(|error| format!("{}: {error}", archive.display()))?
        .len();
    println!(
        "\n{}  {} bytes ({} MiB), sha256 {}",
        archive.display(),
        bytes,
        bytes / 1_048_576,
        digest_file(&archive)?
    );
    Ok(())
}

fn build(config: &Config, table: &[capabilities::Module]) -> Result<PathBuf, String> {
    let target_dir = target_dir();
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
    if !config.host {
        command.args(["--target", &config.target]);
    }
    // The template profile: `crates/thinpack-cli/src/templates.rs` rebuilds templates with the
    // same flags, so a downloaded pair and a locally rebuilt one agree.
    command.env(
        "RUSTFLAGS",
        appended(env::var("RUSTFLAGS").ok(), &rust_flags(&config.target)),
    );
    // MSVC has no gcc-style unwind flags; everything else is clang or gcc.
    if !config.target.contains("msvc") {
        command.env("CFLAGS", appended(env::var("CFLAGS").ok(), C_FLAGS));
    }

    let status = command
        .status()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if !status.success() {
        return Err(format!("build failed ({status})"));
    }
    Ok(if config.host {
        target_dir.join("release")
    } else {
        target_dir.join(&config.target).join("release")
    })
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

    let scratch = target_dir().join("probe");
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
            .args(["pack", PROBE, "-o"])
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
    fs::write(&sums, format!("{}  {stem}.tar.gz\n", digest_file(&path)?))
        .map_err(|error| format!("{}: {error}", sums.display()))?;
    Ok(path)
}

impl Config {
    /// `thinpack-<version>-<triple>`: the staging directory, the archive stem, and the directory
    /// inside the archive.
    fn stem(&self) -> String {
        format!("thinpack-{}-{}", env!("CARGO_PKG_VERSION"), self.target)
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

/// Cargo's target directory for this workspace, with `dist` below it. `CARGO_TARGET_DIR` wins
/// because it is the documented way to build this workspace without the checked-in `target`
/// symlink; nothing is written inside the repository then.
/// MSVC rejects `-C force-unwind-tables=no` ("target requires unwind tables") and has no gcc
/// driver flags at all; both code paths below are the ones that actually work per target.
fn rust_flags(target: &str) -> String {
    if target.contains("msvc") {
        return String::new();
    }
    if target.contains("linux") && target.contains("x86_64") {
        return format!("{UNWIND} {PACK_RELATIVE_RELOCS}");
    }
    UNWIND.to_owned()
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| crate::repo_root().join("target"), PathBuf::from)
        .join("dist")
}

fn host_triple() -> String {
    let output = Command::new("rustc")
        .args(["-vV"])
        .output()
        .expect("rustc is on PATH");
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .expect("rustc -vV reports a host triple")
}

fn appended(existing: Option<String>, extra: &str) -> String {
    match existing {
        Some(existing) if !existing.trim().is_empty() => format!("{existing} {extra}"),
        _ => extra.to_owned(),
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

fn digest_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}
