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
//!
//! Each archive gets a `thinpack-<stem>.release.toml` fragment with one `[[asset]]` table: the
//! archive name, backend, target and, for a glibc Linux target, the oldest glibc the pair needs.
//! The fragments are the release manifest once concatenated, and `scripts/install.sh` reads it
//! instead of taking the archive layout and the glibc floor apart a second time.

use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;

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
    let release = release_metadata(config, &stage, &archive)?;

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
    println!("metadata {}", release.display());
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

/// One `[[asset]]` table, the unit the release job concatenates into the release manifest.
#[derive(Serialize)]
struct Release {
    asset: [Asset; 1],
}

/// What the installer would otherwise have to re-derive: the archive's name and backend, the
/// target it was built for, and the glibc floor only the builder can measure.
#[derive(Serialize)]
struct Asset {
    archive: String,
    backend: &'static str,
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_glibc: Option<String>,
}

fn release_metadata(config: &Config, stage: &Path, archive: &Path) -> Result<PathBuf, String> {
    let name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{}: the archive name is not UTF-8", archive.display()))?
        .to_owned();
    let min_glibc = if config.target.ends_with("-linux-gnu") {
        Some(glibc_floor(config, stage)?)
    } else {
        None
    };
    let body = toml::to_string(&Release {
        asset: [Asset {
            archive: name,
            backend: config.backend.name(),
            target: config.target.clone(),
            min_glibc,
        }],
    })
    .map_err(|error| format!("cannot encode the release metadata: {error}"))?;

    let path = config.out.join(format!("{}.release.toml", config.stem()));
    fs::write(&path, body).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

/// The oldest glibc both staged binaries run on, read from the version requirements the linker
/// recorded in them. The builder's own glibc is not the answer: a runner one release newer links
/// most symbols against the versions it always did, and a floor that moved with the image is what
/// let `install.sh` claim an archive ran where it did not.
fn glibc_floor(config: &Config, stage: &Path) -> Result<String, String> {
    let mut floor = (0, 0, 0);
    for name in BINARIES {
        let binary = stage.join(config.file_name(name, true));
        let mut readelf = Command::new("readelf");
        readelf.arg("--version-info").arg(&binary);
        let dump = output(&mut readelf)
            .map_err(|error| format!("cannot measure the glibc floor: {error}"))?;
        let required = glibc_requirements(&dump).ok_or_else(|| {
            format!(
                "{}: readelf reports no GLIBC_ version requirement; the glibc floor is unknown",
                binary.display()
            )
        })?;
        floor = floor.max(required);
    }
    Ok(version_text(floor))
}

/// The highest `GLIBC_<major>.<minor>[.<patch>]` a `readelf --version-info` dump names. Every
/// version a binary needs appears as `Name: GLIBC_2.34` in the version needs section, behind an
/// offset column; `GLIBC_PRIVATE` and `GLIBCXX_*` do not parse as versions.
fn glibc_requirements(dump: &str) -> Option<(u32, u32, u32)> {
    dump.lines()
        .filter_map(|line| line.split_once("Name: GLIBC_").map(|(_, rest)| rest))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter_map(parse_version)
        .max()
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().map_or(Some(0), |patch| patch.parse().ok())?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// `2.39` for a release, `2.2.5` for the components glibc still names that way.
fn version_text((major, minor, patch): (u32, u32, u32)) -> String {
    if patch == 0 {
        format!("{major}.{minor}")
    } else {
        format!("{major}.{minor}.{patch}")
    }
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

#[cfg(test)]
mod tests {
    use super::{glibc_requirements, parse_version, version_text};

    /// Abridged `readelf --version-info` output of a Rust binary: version symbols, then the
    /// version needs section whose `Name:` lines carry the requirements.
    const DUMP: &str = "\
Version symbols section '.gnu.version' contains 3 entries:
 Addr: 0x00000000000016fe  Offset: 0x000016fe  Link: 4 (.dynsym)
  000:   0 (*local*)       2 (GLIBC_2.3)     3 (GLIBC_2.34)
  004:   3 (GLIBC_2.34)    4 (GLIBCXX_3.4.30) 6 (GLIBC_2.38)

Version needs section '.gnu.version_r' contains 3 entries:
  0x0000: Version: 1  File: libgcc_s.so.1  Cnt: 1
  0x0010:   Name: GCC_3.0  Flags: none  Version: 3
  0x0020: Version: 1  File: libc.so.6  Cnt: 4
  0x0030:   Name: GLIBC_2.3  Flags: none  Version: 2
  0x0040:   Name: GLIBC_2.2.5  Flags: none  Version: 3
  0x0050:   Name: GLIBC_2.34  Flags: none  Version: 5
  0x0060:   Name: GLIBC_2.38  Flags: none  Version: 6
  0x0070:   Name: GLIBC_PRIVATE  Flags: none  Version: 7
";

    #[test]
    fn reads_the_highest_glibc_requirement() {
        assert_eq!(glibc_requirements(DUMP), Some((2, 38, 0)));
        assert_eq!(version_text((2, 38, 0)), "2.38");
    }

    #[test]
    fn ignores_libstdcxx_and_unnumbered_versions() {
        // `GLIBCXX_3.4.30` and `GLIBC_PRIVATE` are not libc floors.
        assert_eq!(
            glibc_requirements("   Name: GLIBC_PRIVATE  Flags: none"),
            None
        );
        assert_eq!(
            glibc_requirements("   Name: GLIBCXX_3.4.30  Flags: none"),
            None
        );
    }

    #[test]
    fn compares_glibc_versions_by_component() {
        assert_eq!(parse_version("2.2.5"), Some((2, 2, 5)));
        assert!(parse_version("2.40").unwrap() > parse_version("2.9").unwrap());
        assert_eq!(version_text((2, 2, 5)), "2.2.5");
        assert_eq!(parse_version("2"), None);
    }
}
