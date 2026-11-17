//! Node.js compatibility suite.
//!
//! Ported files live in `tests/node/compat/test/`, mirroring the layout of
//! Node.js' own `test/` directory so they can stay byte-for-byte copies.
//! `tests/node/config.toml` is the manifest: it assigns every file to a module
//! and records why a file is skipped. Only manifest entries run, and the suite
//! refuses to start when a file under `test/parallel` is missing from the
//! manifest, so a copied test cannot silently go unexercised.
//!
//! The target is `harness = false`; the runner is deliberately small and has
//! no opinion about test output. Each file runs in its own `thinpack-rt`
//! process with a private `TEST_SERIAL_ID`, which keeps Node's `common/tmpdir`
//! isolated between concurrent tests.
//!
//! ```text
//! cargo test -p thinpack-rt --test node                     # everything
//! cargo test -p thinpack-rt --test node -- path             # one module
//! cargo test -p thinpack-rt --test node -- path::test-path  # one file
//! cargo test -p thinpack-rt --test node -- --list
//! ```

#![allow(
    unused_crate_dependencies,
    reason = "this target runs the runtime binary and does not use the package's dependencies"
)]

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

/// A test that hangs must not hang the whole suite. Node's runner and Deno's
/// node compatibility suite both use per-test timeouts; individual files can
/// raise this in the manifest.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Captured output is only ever shown for failures, and trunctated so one
/// assertion storm cannot bury the rest of the run.
const OUTPUT_LIMIT: usize = 32 * 1024;

fn main() -> ExitCode {
    let cli = match Cli::parse(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("{error}\n\n{}", Cli::USAGE);
            return ExitCode::from(2);
        }
    };

    let suite = match Suite::load() {
        Ok(suite) => suite,
        Err(error) => {
            eprintln!("node suite: {error}");
            return ExitCode::from(2);
        }
    };

    if cli.list {
        suite.print_list(&cli);
        return ExitCode::SUCCESS;
    }

    if suite.execute(&cli) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(101)
    }
}

struct Cli {
    filter: Option<String>,
    list: bool,
    ignored: bool,
    include_ignored: bool,
    threads: usize,
}

impl Cli {
    const USAGE: &str = "\
Usage: cargo test -p thinpack-rt --test node -- [FILTER] [OPTIONS]

Options:
  --list, --list-tests   list matching tests instead of running them
  --ignored              run only tests marked ignored in the manifest
  --include-ignored      run ignored tests as well
  --test-threads N       number of tests to run in parallel (default: CPU count)
  --nocapture            accepted for cargo compatibility; child output is always captured
  -h, --help             show this help";

    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut cli = Self {
            filter: None,
            list: false,
            ignored: false,
            include_ignored: false,
            threads: thread::available_parallelism().map_or(1, std::num::NonZero::get),
        };

        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--list" | "--list-tests" => cli.list = true,
                "--ignored" => cli.ignored = true,
                "--include-ignored" => cli.include_ignored = true,
                "--nocapture" => {}
                "-h" | "--help" => {
                    println!("{}", Self::USAGE);
                    std::process::exit(0);
                }
                "--test-threads" | "--jobs" => {
                    let value = args.next().ok_or("missing value for --test-threads")?;
                    cli.threads = value
                        .parse()
                        .map_err(|_| format!("invalid thread count: {value}"))?;
                }
                other if let Some(value) = other.strip_prefix("--test-threads=") => {
                    cli.threads = value
                        .parse()
                        .map_err(|_| format!("invalid thread count: {value}"))?;
                }
                other if other.starts_with("--") => {
                    return Err(format!("unknown option: {other}"));
                }
                filter => {
                    if cli.filter.replace(filter.to_owned()).is_some() {
                        return Err("only one filter is supported".to_owned());
                    }
                }
            }
        }

        if cli.threads == 0 {
            return Err("thread count must be at least 1".to_owned());
        }
        Ok(cli)
    }

    fn matches(&self, name: &str) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|filter| name.contains(filter))
    }
}

struct Suite {
    /// `tests/node`, holding the manifest and the mirrored `compat/test` tree.
    root: PathBuf,
    tests: Vec<Test>,
}

struct Test {
    /// `module::file`, e.g. `path::test-path-basename`.
    name: String,
    /// Path relative to `compat/`, e.g. `test/parallel/test-path-basename.js`.
    relative: String,
    ignore: Option<String>,
    timeout: Duration,
    env: BTreeMap<String, String>,
}

impl Suite {
    fn load() -> Result<Self, String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/node");
        let manifest = fs::read_to_string(root.join("config.toml"))
            .map_err(|error| format!("cannot read config.toml: {error}"))?;
        let config: Config = toml::from_str(&manifest)
            .map_err(|error| format!("cannot parse config.toml: {error}"))?;

        let tests_dir = root.join("compat/test");
        let mut tests = Vec::with_capacity(config.tests.len());
        let mut names = HashSet::with_capacity(config.tests.len());

        for (relative, test) in config.tests {
            validate_path(&relative)?;
            let path = tests_dir.join(&relative);
            if !path.is_file() {
                return Err(format!("manifest entry has no file: {relative}"));
            }
            if test.module.is_empty() {
                return Err(format!("manifest entry has no module: {relative}"));
            }
            if let Some(reason) = &test.ignore
                && reason.trim().is_empty()
            {
                return Err(format!("ignored entry has no reason: {relative}"));
            }

            let stem = Path::new(&relative)
                .file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or_else(|| format!("invalid test file name: {relative}"))?;
            let name = format!("{}::{stem}", test.module);
            if !names.insert(name.clone()) {
                return Err(format!("duplicate test name: {name}"));
            }

            tests.push(Test {
                name,
                relative,
                ignore: test.ignore,
                timeout: test
                    .timeout_ms
                    .map_or(DEFAULT_TIMEOUT, Duration::from_millis),
                env: test.env,
            });
        }

        let suite = Self { root, tests };
        suite.check_manifest_is_exhaustive()?;
        Ok(suite)
    }

    /// Every `test-*` file under `parallel/` must be in the manifest. This
    /// catches a copied file that nobody ever runs, which is otherwise very
    /// easy to miss when porting a batch of upstream tests.
    fn check_manifest_is_exhaustive(&self) -> Result<(), String> {
        let dir = self.root.join("compat/test/parallel");
        let mut listed: HashSet<&str> = self
            .tests
            .iter()
            .map(|test| test.relative.as_str())
            .collect();
        let mut unlisted = Vec::new();
        for file in
            test_files(&dir).map_err(|error| format!("cannot scan {}: {error}", dir.display()))?
        {
            let relative = format!("parallel/{file}");
            if !listed.remove(relative.as_str()) {
                unlisted.push(relative);
            }
        }

        if unlisted.is_empty() {
            Ok(())
        } else {
            unlisted.sort();
            Err(format!(
                "test files missing from tests/node/config.toml: {}",
                unlisted.join(", "),
            ))
        }
    }

    fn print_list(&self, cli: &Cli) {
        for test in &self.tests {
            if !cli.matches(&test.name) {
                continue;
            }
            match &test.ignore {
                Some(reason) => println!("{} (ignored: {reason})", test.name),
                None => println!("{}", test.name),
            }
        }
    }

    fn execute(&self, cli: &Cli) -> bool {
        let (mut selected, ignored): (Vec<&Test>, Vec<&Test>) = self
            .tests
            .iter()
            .filter(|test| cli.matches(&test.name))
            .partition(|test| cli.ignored || cli.include_ignored || test.ignore.is_none());

        if cli.ignored {
            selected.retain(|test| test.ignore.is_some());
        }

        if selected.is_empty() {
            println!(
                "running 0 tests ({} ignored)\n\ntest result: ok. 0 passed; 0 failed; {} ignored",
                ignored.len(),
                ignored.len(),
            );
            return true;
        }

        println!(
            "running {} test{} ({} ignored)",
            selected.len(),
            if selected.len() == 1 { "" } else { "s" },
            ignored.len(),
        );

        let started = Instant::now();
        let next = AtomicUsize::new(0);
        let failures = Mutex::new(Vec::<String>::new());
        let output = Mutex::new(());
        let workers = cli.threads.min(selected.len());

        thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(test) = selected.get(index) else {
                            break;
                        };
                        let outcome = run_test(&self.root, test, index);
                        let guard = output.lock().expect("output lock");
                        match outcome {
                            Outcome::Passed => println!("ok {}", test.name),
                            Outcome::Failed(report) => {
                                println!("FAILED {}\n{report}", test.name);
                                failures
                                    .lock()
                                    .expect("failure lock")
                                    .push(test.name.clone());
                            }
                        }
                        drop(guard);
                    }
                });
            }
        });

        let failures = failures.into_inner().expect("failure lock");
        let elapsed = started.elapsed();
        let passed = selected.len() - failures.len();
        let result = if failures.is_empty() { "ok" } else { "FAILED" };
        println!(
            "\ntest result: {result}. {passed} passed; {} failed; {} ignored; finished in {:.2}s",
            failures.len(),
            ignored.len(),
            elapsed.as_secs_f64(),
        );
        failures.is_empty()
    }
}

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    tests: BTreeMap<String, TestConfig>,
}

#[derive(Deserialize)]
struct TestConfig {
    module: String,
    #[serde(default)]
    ignore: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

fn validate_path(relative: &str) -> Result<(), String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        || !relative.starts_with("parallel/")
        || !matches!(
            path.extension().and_then(std::ffi::OsStr::to_str),
            Some("js" | "mjs" | "cjs")
        )
    {
        return Err(format!(
            "manifest key must be a parallel/test-*.js file: {relative}"
        ));
    }
    Ok(())
}

/// File names under `dir`, recursively. Only files that look like tests are
/// returned; fixtures and helper data may live next to them.
fn test_files(dir: &Path) -> io::Result<Vec<String>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            for file in test_files(&path)? {
                files.push(format!("{name}/{file}"));
            }
        } else if name.starts_with("test-")
            && matches!(
                path.extension().and_then(std::ffi::OsStr::to_str),
                Some("js" | "mjs" | "cjs")
            )
        {
            files.push(name.to_owned());
        }
    }
    Ok(files)
}

enum Outcome {
    Passed,
    Failed(String),
}

fn run_test(root: &Path, test: &Test, serial: usize) -> Outcome {
    let mut command = Command::new(env!("CARGO_BIN_EXE_thinpack-rt"));
    command
        .current_dir(root.join("compat"))
        .arg(Path::new("test").join(&test.relative))
        .env("TEST_SERIAL_ID", format!("{}-{serial}", std::process::id()))
        .env("NO_COLOR", "1")
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &test.env {
        command.env(key, value);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Outcome::Failed(format!("cannot spawn thinpack-rt: {error}"));
        }
    };
    let Some(stdout) = child.stdout.take() else {
        return Outcome::Failed("stdout is not piped".to_owned());
    };
    let Some(stderr) = child.stderr.take() else {
        return Outcome::Failed("stderr is not piped".to_owned());
    };
    let stdout = thread::spawn(move || read_all(stdout));
    let stderr = thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + test.timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                return Outcome::Failed(format!("cannot wait for thinpack-rt: {error}"));
            }
        }
        if Instant::now() >= deadline {
            timed_out = true;
            if let Err(error) = child.kill() {
                return Outcome::Failed(format!("cannot kill timed-out test: {error}"));
            }
            match child.wait() {
                Ok(status) => break status,
                Err(error) => {
                    return Outcome::Failed(format!("cannot reap timed-out test: {error}"));
                }
            }
        }
        thread::sleep(Duration::from_millis(25));
    };

    let stdout = read(stdout);
    let stderr = read(stderr);

    if status.success() && !timed_out {
        return Outcome::Passed;
    }

    let reason = if timed_out {
        format!("timed out after {} ms", test.timeout.as_millis())
    } else {
        format!("exited with {}", describe_status(status))
    };
    let mut report = format!("{reason}\n");
    if !stdout.is_empty() {
        report.push_str("\n----- stdout -----\n");
        report.push_str(&truncate(&stdout));
    }
    if !stderr.is_empty() {
        report.push_str("\n----- stderr -----\n");
        report.push_str(&truncate(&stderr));
    }
    if !report.ends_with('\n') {
        report.push('\n');
    }
    Outcome::Failed(report)
}

fn read(handle: thread::JoinHandle<io::Result<String>>) -> String {
    match handle.join() {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => format!("<cannot read output: {error}>"),
        Err(_) => "<output reader panicked>".to_owned(),
    }
}

fn read_all(mut source: impl Read) -> io::Result<String> {
    let mut buffer = Vec::new();
    source.read_to_end(&mut buffer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

fn describe_status(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| status.to_string(), |code| format!("exit code {code}"))
}

fn truncate(text: &str) -> String {
    if text.len() <= OUTPUT_LIMIT {
        return text.to_owned();
    }
    let mut end = OUTPUT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n... {} bytes omitted ...\n",
        &text[..end],
        text.len() - end,
    )
}
