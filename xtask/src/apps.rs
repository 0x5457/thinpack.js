//! `apps`: pack the programs the README compares, measure the artifacts, and hold them to a
//! committed baseline.
//!
//! Artifact size is this project's headline claim, so it gets the same treatment as a test:
//! one fixture per program under `tests/apps/<name>`, dependencies pinned by a lockfile, and
//! a baseline a regression has to update deliberately. Every app is packed with `--rebuild`,
//! so it carries a template built for its own capability closure — the smallest artifact the
//! CLI can produce, and the size the README lists.
//!
//! The fixtures are installed with `npm ci`, and the report needs `gzip`: the command runs on
//! Linux and macOS.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde::{Deserialize, Serialize};

use crate::measure::{gzip_size, mib};

const USAGE: &str = "\
Usage: cargo xtask apps [--only <name,...>] [--baseline <path>] [--write-baseline]
                        [--summary <path>]";

const DEFAULT_BASELINE: &str = "tests/apps/baseline.toml";
const FIXTURES: &str = "tests/apps";

/// `--rebuild` uses this backend and the small profile (`templates.rs`); the baseline records
/// both so a run on another backend or profile can never be compared against it.
const BACKEND: &str = thinpack_pack::Backend::QuickJs.name();
const PROFILE: &str = "small";

/// Growth allowed before the gate fails. Sizes are reproducible for a fixed input, but the
/// runner image updates under a pinned commit; one percent separates that noise from a real
/// regression.
const MAX_GROWTH_PERCENT: u64 = 1;

const BASELINE_HEADER: &str = "\
# Artifact sizes of the apps in the README, recorded by `cargo xtask apps --write-baseline`.
# The Size workflow fails when an artifact exceeds the recorded size by more than 1 percent;
# re-record after an intentional change and review the diff.
";

/// The programs in the README's comparison tables, in their order.
const APPS: &[App] = &[
    App {
        name: "tsc",
        package: "typescript",
        entry: None,
        includes: &[Include::Declarations("node_modules/typescript/lib")],
    },
    App {
        name: "prettier",
        package: "prettier",
        entry: None,
        includes: &[Include::Path("node_modules/prettier/internal")],
    },
    App {
        name: "eslint",
        package: "eslint",
        entry: None,
        // The default formatter is resolved by path at run time, not imported statically.
        includes: &[Include::Path(
            "node_modules/eslint/lib/cli-engine/formatters",
        )],
    },
    App {
        name: "terser",
        package: "terser",
        entry: None,
        includes: &[],
    },
    App {
        name: "js-yaml",
        package: "js-yaml",
        entry: None,
        includes: &[],
    },
    App {
        name: "pi",
        package: "@earendil-works/pi-coding-agent",
        // The package's `bin` is a two-line launcher that `require()`s a second ESM bundle;
        // `dist/cli.js` is the statically linked CLI and the closure the README reports.
        entry: Some("dist/cli.js"),
        includes: &[],
    },
];

struct App {
    /// Fixture directory and artifact name.
    name: &'static str,
    package: &'static str,
    /// Entry relative to the package directory; `None` uses the package's `bin` command named
    /// after the app.
    entry: Option<&'static str>,
    /// Paths under the fixture that hold data the analyzer cannot see.
    includes: &'static [Include],
}

enum Include {
    /// Passed to `--include` as is: a file, or a directory that is analyzed as JavaScript too.
    Path(&'static str),
    /// Only the `.d.ts` files of a directory. TypeScript's `lib/` also holds its compiler API
    /// and the server entries, and analyzing those would put modules the CLI never loads into
    /// the template.
    Declarations(&'static str),
}

struct Config {
    /// `--only`: measure a subset against the full baseline, for a quick rerun after a change.
    only: Option<Vec<&'static App>>,
    baseline: PathBuf,
    write_baseline: bool,
    summary: Option<PathBuf>,
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    match parse(args).and_then(|config| measure_all(&config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[String]) -> Result<Config, String> {
    let mut only = None;
    let mut baseline = crate::repo_root().join(DEFAULT_BASELINE);
    let mut write_baseline = false;
    let mut summary = None;

    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--only" => only = Some(select(value_of(&mut args, arg)?)?),
            "--baseline" => baseline = PathBuf::from(value_of(&mut args, arg)?),
            "--write-baseline" => write_baseline = true,
            "--summary" => summary = Some(PathBuf::from(value_of(&mut args, arg)?)),
            _ => return Err(format!("unknown argument `{arg}`\n{USAGE}")),
        }
    }

    if write_baseline && only.is_some() {
        return Err("--write-baseline records every app; rerun without --only".to_owned());
    }

    Ok(Config {
        only,
        baseline,
        write_baseline,
        summary,
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

fn select(value: &str) -> Result<Vec<&'static App>, String> {
    value
        .split(',')
        .map(|name| {
            let name = name.trim();
            APPS.iter().find(|app| app.name == name).ok_or_else(|| {
                let known: Vec<&str> = APPS.iter().map(|app| app.name).collect();
                format!("unknown app `{name}`; expected {}", known.join(", "))
            })
        })
        .collect()
}

fn measure_all(config: &Config) -> Result<(), String> {
    let host = crate::build::host_triple()?;
    build_cli()?;

    let apps: Vec<&'static App> = config.only.clone().unwrap_or_else(|| APPS.iter().collect());
    let mut measurements = Vec::with_capacity(apps.len());
    for app in apps {
        // Template builds take minutes and their output is captured; one line per app keeps
        // a CI log readable while the measurement runs.
        eprintln!("measuring {}", app.name);
        measurements.push((app, measure(app)?));
    }

    let baseline = if config.write_baseline {
        None
    } else {
        Some(read_baseline(&config.baseline)?)
    };
    let comparable = baseline.as_ref().filter(|baseline| baseline.matches(&host));
    let regressions = baseline
        .as_ref()
        .map_or_else(Vec::new, |baseline| compare(baseline, &host, &measurements));
    let markdown = report(config, &host, &measurements, comparable, &regressions);

    print!("{markdown}");
    if let Some(summary) = &config.summary {
        append_summary(summary, &markdown)?;
    }
    if config.write_baseline {
        write_baseline(&config.baseline, &host, &measurements)?;
    }
    if regressions.is_empty() {
        Ok(())
    } else {
        Err(regressions.join("\n"))
    }
}

fn build_cli() -> Result<(), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(&cargo)
        .current_dir(crate::repo_root())
        .args(["build", "--release", "--locked", "-p", "thinpack-cli"])
        .status()
        .map_err(|error| format!("cannot run `{cargo}`: {error}"))?;
    if !status.success() {
        return Err(format!("building thinpack-cli failed ({status})"));
    }
    Ok(())
}

fn cli_path() -> PathBuf {
    crate::build::target_root()
        .join("release")
        .join(format!("thinpack{}", env::consts::EXE_SUFFIX))
}

fn artifact_root() -> PathBuf {
    crate::build::target_root().join("apps")
}

fn measure(app: &'static App) -> Result<Record, String> {
    let fixture = crate::repo_root().join(FIXTURES).join(app.name);
    install(&fixture)?;
    let package = installed(app, &fixture)?;

    let artifact = artifact_root().join(app.name);
    if let Some(directory) = artifact.parent() {
        fs::create_dir_all(directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?;
    }

    let includes = include_paths(app, &fixture)?;
    pack(&fixture, &package.entry, &artifact, &includes)?;
    let summary = PackSummary::read(&artifact)?;
    let bytes = fs::metadata(&artifact)
        .map_err(|error| format!("{}: {error}", artifact.display()))?
        .len();
    verify(&artifact, &package.version)?;

    Ok(Record {
        package: app.package.to_owned(),
        version: package.version,
        bytes,
        gzip_bytes: gzip_size(&artifact)?,
        payload_bytes: summary.payload_bytes,
        files: summary.files,
        capabilities: summary.capabilities,
    })
}

/// The fixtures need no install scripts; skipping them keeps the package contents identical
/// between a workstation and the runner.
fn install(fixture: &Path) -> Result<(), String> {
    let output = Command::new("npm")
        .current_dir(fixture)
        .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"])
        .output()
        .map_err(|error| {
            format!("cannot run npm: {error}; the app fixtures are installed with npm")
        })?;
    if !output.status.success() {
        return Err(format!(
            "npm ci in {} failed ({}):\n{}",
            fixture.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

struct Package {
    version: String,
    entry: PathBuf,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    bin: Option<Bin>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Bin {
    Command(String),
    Commands(BTreeMap<String, String>),
}

fn installed(app: &App, fixture: &Path) -> Result<Package, String> {
    let directory = fixture.join("node_modules").join(app.package);
    let path = directory.join("package.json");
    let text = fs::read_to_string(&path).map_err(|error| {
        format!(
            "{}: {error}; run `npm ci` in {}",
            path.display(),
            fixture.display()
        )
    })?;
    let manifest: Manifest =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;

    let entry = match app.entry {
        Some(entry) => directory.join(entry),
        None => directory.join(bin_entry(app, manifest.bin)?),
    };
    Ok(Package {
        version: manifest.version,
        entry,
    })
}

fn bin_entry(app: &App, bin: Option<Bin>) -> Result<String, String> {
    match bin {
        Some(Bin::Command(command)) => Ok(command),
        Some(Bin::Commands(mut commands)) => commands.remove(app.name).ok_or_else(|| {
            format!(
                "{} has no `{}` in `bin`; set `entry` for the app",
                app.package, app.name
            )
        }),
        None => Err(format!(
            "{} has no `bin`; set `entry` for the app",
            app.package
        )),
    }
}

fn include_paths(app: &App, fixture: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for include in app.includes {
        match include {
            Include::Path(path) => paths.push(fixture.join(path)),
            Include::Declarations(path) => {
                let directory = fixture.join(path);
                let mut declarations = Vec::new();
                for entry in fs::read_dir(&directory)
                    .map_err(|error| format!("{}: {error}", directory.display()))?
                {
                    let path = entry
                        .map_err(|error| format!("{}: {error}", directory.display()))?
                        .path();
                    if path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().ends_with(".d.ts"))
                    {
                        declarations.push(path);
                    }
                }
                if declarations.is_empty() {
                    return Err(format!("no `.d.ts` files in {}", directory.display()));
                }
                declarations.sort();
                paths.extend(declarations);
            }
        }
    }
    Ok(paths)
}

fn pack(
    fixture: &Path,
    entry: &Path,
    artifact: &Path,
    includes: &[PathBuf],
) -> Result<String, String> {
    let cli = cli_path();
    let mut command = Command::new(&cli);
    command
        .current_dir(fixture)
        .arg("pack")
        .arg(entry)
        .arg("-o")
        .arg(artifact)
        .args(["--rebuild", "--source"])
        .arg(crate::repo_root());
    for include in includes {
        command.arg("--include").arg(include);
    }

    let output = command
        .output()
        .map_err(|error| format!("cannot run {}: {error}", cli.display()))?;
    if !output.status.success() {
        return Err(format!(
            "packing {} failed ({}):\n{}",
            entry.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

struct PackSummary {
    files: u64,
    capabilities: u64,
    payload_bytes: u64,
}

impl PackSummary {
    /// Decode the payload the same way the runtime does instead of parsing the CLI's
    /// human-readable summary.
    fn read(artifact: &Path) -> Result<Self, String> {
        let bytes =
            fs::read(artifact).map_err(|error| format!("{}: {error}", artifact.display()))?;
        let trailer_start = bytes
            .len()
            .checked_sub(thinpack_pack::TRAILER_LEN)
            .ok_or_else(|| format!("{}: no thinpack trailer", artifact.display()))?;
        let payload_len = thinpack_pack::trailer_payload_len(&bytes[trailer_start..])
            .ok_or_else(|| format!("{}: no thinpack trailer", artifact.display()))?;
        let payload_len = usize::try_from(payload_len)
            .map_err(|_| format!("{}: payload is too large", artifact.display()))?;
        let payload_start = trailer_start
            .checked_sub(payload_len)
            .ok_or_else(|| format!("{}: truncated payload", artifact.display()))?;
        let pack = thinpack_pack::Pack::read(&bytes[payload_start..trailer_start])
            .map_err(|error| format!("{}: {error}", artifact.display()))?;
        Ok(Self {
            files: pack.manifest.files.len() as u64,
            capabilities: pack.manifest.capabilities.len() as u64,
            payload_bytes: payload_len as u64,
        })
    }
}

fn verify(artifact: &Path, version: &str) -> Result<(), String> {
    let output = Command::new(artifact)
        .arg("--version")
        .output()
        .map_err(|error| format!("cannot run {}: {error}", artifact.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !stdout.contains(version) {
        return Err(format!(
            "{} --version did not report {version} ({}):\nstdout: {}\nstderr: {}",
            artifact.display(),
            output.status,
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[derive(Clone, Deserialize, Serialize)]
struct Record {
    package: String,
    version: String,
    bytes: u64,
    gzip_bytes: u64,
    payload_bytes: u64,
    files: u64,
    capabilities: u64,
}

#[derive(Deserialize, Serialize)]
struct Baseline {
    target: String,
    backend: String,
    profile: String,
    apps: BTreeMap<String, Record>,
}

impl Baseline {
    /// Sizes from another target, backend or profile are not comparable; the gate says so
    /// instead of showing a delta that means nothing.
    fn matches(&self, host: &str) -> bool {
        self.target == host && self.backend == BACKEND && self.profile == PROFILE
    }
}

fn read_baseline(path: &Path) -> Result<Baseline, String> {
    let text = fs::read_to_string(path).map_err(|error| {
        format!(
            "{}: {error}\nrecord the current sizes with `cargo xtask apps --write-baseline`",
            path.display()
        )
    })?;
    toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_baseline(
    path: &Path,
    host: &str,
    measurements: &[(&'static App, Record)],
) -> Result<(), String> {
    let baseline = Baseline {
        target: host.to_owned(),
        backend: BACKEND.to_owned(),
        profile: PROFILE.to_owned(),
        apps: measurements
            .iter()
            .map(|(app, record)| ((*app.name).to_owned(), record.clone()))
            .collect(),
    };
    let body = toml::to_string(&baseline)
        .map_err(|error| format!("cannot encode the baseline: {error}"))?;
    fs::write(path, format!("{BASELINE_HEADER}{body}"))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    println!(
        "recorded {} apps in {}",
        measurements.len(),
        repo_relative(path).display()
    );
    Ok(())
}

fn compare(
    baseline: &Baseline,
    host: &str,
    measurements: &[(&'static App, Record)],
) -> Vec<String> {
    if !baseline.matches(host) {
        return vec![format!(
            "baseline is {BACKEND} {PROFILE} on {}; this run measured {BACKEND} {PROFILE} on {host}. \
             Re-record it on the reference runner",
            baseline.target
        )];
    }

    let mut regressions = Vec::new();
    for (app, record) in measurements {
        let Some(expected) = baseline.apps.get(app.name) else {
            regressions.push(format!(
                "{}: not in the baseline; run `cargo xtask apps --write-baseline`",
                app.name
            ));
            continue;
        };
        if expected.version != record.version {
            regressions.push(format!(
                "{}: fixture moved from {} to {}; re-record the baseline",
                app.name, expected.version, record.version
            ));
            continue;
        }
        growth(
            app.name,
            "artifact",
            record.bytes,
            expected.bytes,
            &mut regressions,
        );
        growth(
            app.name,
            "gzip",
            record.gzip_bytes,
            expected.gzip_bytes,
            &mut regressions,
        );
    }
    regressions
}

fn growth(app: &str, what: &str, measured: u64, baseline: u64, regressions: &mut Vec<String>) {
    let limit = baseline + baseline * MAX_GROWTH_PERCENT / 100;
    if measured > limit {
        regressions.push(format!(
            "{app}: {what} is {:.2} MiB, {:.1}% over the baseline {:.2} MiB (limit {MAX_GROWTH_PERCENT}%)",
            mib(measured),
            percent(measured, baseline),
            mib(baseline)
        ));
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "measured artifacts stay far below 2^53 bytes, where `u64` → `f64` is exact"
)]
fn percent(measured: u64, baseline: u64) -> f64 {
    (measured as f64 - baseline as f64) / baseline as f64 * 100.0
}

/// A path below the checkout is reported relative to it: the absolute CI prefix is noise.
fn repo_relative(path: &Path) -> &Path {
    path.strip_prefix(crate::repo_root()).unwrap_or(path)
}

fn report(
    config: &Config,
    host: &str,
    measurements: &[(&'static App, Record)],
    baseline: Option<&Baseline>,
    regressions: &[String],
) -> String {
    let mut markdown = String::from("### Packed app artifacts\n\n");
    let _ = write!(markdown, "{BACKEND} backend, {PROFILE} profile, `{host}`");
    if !config.write_baseline {
        let _ = write!(
            markdown,
            ", baseline `{}`",
            repo_relative(&config.baseline).display()
        );
    }
    markdown.push_str(
        "\n\n| App | Version | Artifact | gzip | Payload | Files | Caps | vs baseline |\n\
         | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for (app, record) in measurements {
        let delta = baseline
            .and_then(|baseline| baseline.apps.get(app.name))
            .map_or_else(
                || "—".to_owned(),
                |expected| format!("{:+.1}%", percent(record.bytes, expected.bytes)),
            );
        let _ = writeln!(
            markdown,
            "| `{}` | {} | {:.2} MiB | {:.2} MiB | {:.2} MiB | {} | {} | {delta} |",
            app.name,
            record.version,
            mib(record.bytes),
            mib(record.gzip_bytes),
            mib(record.payload_bytes),
            record.files,
            record.capabilities
        );
    }
    if !regressions.is_empty() {
        markdown.push_str("\n**Size regression:**\n");
        for regression in regressions {
            let _ = writeln!(markdown, "\n- {regression}");
        }
    }
    markdown
}

fn append_summary(path: &Path, markdown: &str) -> Result<(), String> {
    use std::io::Write as _;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.write_all(markdown.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{APPS, App, Baseline, PackSummary, Record, compare};

    const HOST: &str = "x86_64-unknown-linux-gnu";

    fn record(version: &str, bytes: u64, gzip_bytes: u64) -> Record {
        Record {
            package: APPS[0].package.to_owned(),
            version: version.to_owned(),
            bytes,
            gzip_bytes,
            payload_bytes: 0,
            files: 0,
            capabilities: 0,
        }
    }

    fn baseline(app: &App, record: Record) -> Baseline {
        Baseline {
            target: HOST.to_owned(),
            backend: "quickjs".to_owned(),
            profile: "small".to_owned(),
            apps: [(app.name.to_owned(), record)].into(),
        }
    }

    #[test]
    fn reads_the_summary_from_a_payload() {
        let manifest = thinpack_pack::Manifest {
            entry: "/app/main.js".to_owned(),
            capabilities: vec!["assert".to_owned(), "fs".to_owned()],
            conditions: Vec::new(),
            template: thinpack_pack::Template {
                version: "1.0.0".to_owned(),
                backend: thinpack_pack::Backend::QuickJs,
                capabilities_hash: 0,
            },
            files: vec![thinpack_pack::FileRecord {
                path: "/app/main.js".to_owned(),
                offset: 0,
                length: 0,
            }],
            formats: Vec::new(),
            assets: Vec::new(),
        };
        let payload = thinpack_pack::assemble(&manifest, b"", thinpack_pack::Compression::None)
            .expect("assemble");
        let binary = thinpack_pack::append_to_binary(b"", &payload).expect("append");
        let path =
            std::env::temp_dir().join(format!("thinpack-apps-summary-{}", std::process::id()));
        fs::write(&path, binary).expect("write the artifact");

        let summary = PackSummary::read(&path).expect("summary");
        assert_eq!(summary.files, 1);
        assert_eq!(summary.capabilities, 2);
        assert_eq!(summary.payload_bytes, payload.len() as u64);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_a_file_without_a_trailer() {
        let path =
            std::env::temp_dir().join(format!("thinpack-apps-no-trailer-{}", std::process::id()));
        fs::write(&path, b"not a packed executable").expect("write the file");
        assert!(PackSummary::read(&path).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn gate_allows_exactly_one_percent_growth() {
        let app = &APPS[0];
        let baseline = baseline(app, record("1.0.0", 1_000, 1_000));

        assert_eq!(
            compare(&baseline, HOST, &[(app, record("1.0.0", 1_010, 1_010))]),
            Vec::<String>::new()
        );
        assert_eq!(
            compare(&baseline, HOST, &[(app, record("1.0.0", 1_011, 1_010))]).len(),
            1
        );
    }

    #[test]
    fn gate_rejects_a_moved_fixture() {
        let app = &APPS[0];
        let baseline = baseline(app, record("1.0.0", 1_000, 1_000));

        let regressions = compare(&baseline, HOST, &[(app, record("1.1.0", 1_000, 1_000))]);
        assert_eq!(regressions.len(), 1);
        assert!(regressions[0].contains("fixture moved"));
    }

    #[test]
    fn gate_rejects_another_environment() {
        let app = &APPS[0];
        let baseline = baseline(app, record("1.0.0", 1_000, 1_000));

        let regressions = compare(
            &baseline,
            "aarch64-apple-darwin",
            &[(app, record("1.0.0", 1_000, 1_000))],
        );
        assert_eq!(regressions.len(), 1);
        assert!(regressions[0].contains("aarch64-apple-darwin"));
    }
}
