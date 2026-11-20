#![allow(
    unused_crate_dependencies,
    reason = "these integration cases invoke the CLI binary and inspect packs; other dependencies belong to the CLI implementation"
)]

#[cfg(unix)]
mod unix {
    use std::fmt::Write as _;
    use std::fs;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

    #[test]
    fn cjs_top_level_return_can_be_packed() {
        let directory = TempDir::new("cjs-top-level-return");
        let entry = directory.path().join("main.cjs");
        let output = directory.path().join("packed-app");
        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let cli = Path::new(env!("CARGO_BIN_EXE_thinpack"));
        let source = "return;\nmodule.exports = 'after return';\n";
        fs::write(&entry, source).expect("write CJS entry");

        let mut command = Command::new(cli);
        command
            .current_dir(directory.path())
            .env("THINPACK_CACHE", directory.path().join("cache"))
            .args(["pack"])
            .arg(&entry)
            .arg("--template")
            .arg(&template)
            .arg("--output")
            .arg(&output);
        let result = run_with_timeout(&mut command);
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert_eq!(pack.manifest.entry, "/app/main.cjs");
        let entry = pack
            .manifest
            .files
            .iter()
            .find(|file| file.path == "/app/main.cjs")
            .expect("packed CJS entry");
        assert_eq!(pack.slice_of(entry), Some(source.as_bytes()));
    }

    #[test]
    fn finite_conditional_require_is_complete_in_strict_mode() {
        let directory = TempDir::new("finite-conditional-require");
        let entry = directory.path().join("main.js");
        fs::write(
            &entry,
            r"
const loadBuiltin = require;
const networkModule = 'node:dns';
module.exports = function loadSystemModule(networkEnabled) {
  var selected = networkEnabled ? networkModule : 'node:os';
  return loadBuiltin(selected);
};
",
        )
        .expect("write conditional require entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &["--list-capabilities", "--strict"],
        );
        assert!(
            result.status.success(),
            "strict capability listing failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let listed = String::from_utf8_lossy(&result.stdout);
        assert!(
            listed.lines().any(|line| line.starts_with("dns ")),
            "{listed}"
        );
        assert!(
            listed.lines().any(|line| line.starts_with("os ")),
            "{listed}"
        );
        assert!(
            !String::from_utf8_lossy(&result.stderr).contains("non-literal argument"),
            "unexpected warning: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn truly_dynamic_require_remains_a_strict_error() {
        let directory = TempDir::new("dynamic-require-strict");
        let entry = directory.path().join("main.js");
        fs::write(
            &entry,
            "const selected = process.argv[2];\nrequire(selected);\n",
        )
        .expect("write dynamic require entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &["--list-capabilities", "--strict"],
        );
        assert!(
            !result.status.success(),
            "dynamic require passed strict mode"
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("non-literal argument cannot be analyzed"),
            "{error}"
        );
        assert!(
            error.contains("--strict turns this warning into an error"),
            "{error}"
        );
    }

    #[test]
    fn comma_delimited_with_accumulates_capabilities() {
        let directory = TempDir::new("comma-with");
        let entry = directory.path().join("main.js");
        fs::write(&entry, "module.exports = 1;\n").expect("write entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--list-capabilities",
                "--with",
                "zlib,os",
                "--with",
                "vm,worker_threads",
                "--with",
                "async_hooks",
            ],
        );
        assert!(
            result.status.success(),
            "capability listing failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let listed = String::from_utf8_lossy(&result.stdout);
        for module in ["zlib", "os", "vm", "worker_threads", "async_hooks"] {
            assert!(
                listed
                    .lines()
                    .any(|line| line.starts_with(&format!("{module} "))),
                "missing {module}:\n{listed}"
            );
        }
    }

    #[test]
    fn comma_delimited_overrides_report_unknown_and_empty_names() {
        let directory = TempDir::new("comma-override-diagnostics");
        let entry = directory.path().join("main.js");
        fs::write(&entry, "module.exports = 1;\n").expect("write entry");
        let cli = Path::new(env!("CARGO_BIN_EXE_thinpack"));

        let unknown_with = run_pack(
            cli,
            directory.path(),
            &entry,
            &[
                "--list-capabilities",
                "--with",
                "dns,unknown_with_module",
                "--with",
                "os",
            ],
        );
        assert!(!unknown_with.status.success());
        let error = String::from_utf8_lossy(&unknown_with.stderr);
        assert!(
            error.contains("unknown module") && error.contains("unknown_with_module"),
            "{error}"
        );
        assert!(!error.contains("dns,unknown_with_module"), "{error}");
        assert!(error.contains("thinpack capabilities"), "{error}");

        let unknown_without = run_pack(
            cli,
            directory.path(),
            &entry,
            &[
                "--list-capabilities",
                "--without",
                "unknown_without_module,dns",
                "--without",
                "os,ffi",
            ],
        );
        assert!(!unknown_without.status.success());
        let error = String::from_utf8_lossy(&unknown_without.stderr);
        assert!(
            error.contains("unknown module") && error.contains("unknown_without_module"),
            "{error}"
        );
        assert!(!error.contains("unknown_without_module,dns"), "{error}");
        assert!(error.contains("thinpack capabilities"), "{error}");

        for (option, value) in [("--with", "dns,,os"), ("--without", ",dns")] {
            let result = run_pack(
                cli,
                directory.path(),
                &entry,
                &["--list-capabilities", option, value],
            );
            assert!(
                !result.status.success(),
                "accepted empty item in {option} {value}"
            );
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(error.contains(option), "{error}");
            assert!(error.contains("empty module name"), "{error}");
            assert!(error.contains(value), "{error}");
        }
    }

    #[test]
    fn capabilities_command_lists_the_cli_catalog() {
        let directory = TempDir::new("capabilities-catalog");
        let result = run_cli(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &["capabilities"],
        );
        assert!(result.status.success());
        let listed = String::from_utf8_lossy(&result.stdout);
        let names: Vec<_> = listed.lines().collect();
        let expected: Vec<_> = thinpack_caps::MODULES
            .iter()
            .map(|module| module.name)
            .collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn explicit_template_backend_must_match_request() {
        let directory = TempDir::new("explicit-template-backend");
        let entry = directory.path().join("main.js");
        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        fs::write(&entry, "module.exports = 1;\n").expect("write entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--backend",
                "v8",
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("uses the quickjs backend, but --backend v8 was requested"),
            "{error}"
        );
        assert!(!output.exists(), "wrote a pack from a mismatched template");
    }

    #[test]
    fn cache_does_not_select_a_template_from_another_version() {
        let directory = TempDir::new("stale-template-version");
        // Avoid the CLI's sibling-runtime fallback so only the cache is tested.
        let cli = copy_cli(&directory);
        let entry = directory.path().join("main.js");
        let output = directory.path().join("packed-app");
        let cache = directory.path().join("cache");
        let stale = cache
            .join("templates")
            .join("0.0.0")
            .join("quickjs")
            .join("0000000000000000");
        fs::create_dir_all(&stale).expect("create stale template cache entry");
        fs::write(&entry, "module.exports = 1;\n").expect("write app entry");
        // A full capability list ensures the candidate reaches version matching.
        fake_template(&stale.join("thinpack-rt"), "0.0.0");
        let capabilities = thinpack_caps::MODULES
            .iter()
            .map(|module| module.name)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(stale.join("capabilities"), format!("{capabilities}\n"))
            .expect("write cached capabilities");

        let mut command = Command::new(cli);
        command
            .current_dir(directory.path())
            .env("THINPACK_CACHE", &cache)
            .args(["pack"])
            .arg(&entry)
            .arg("--output")
            .arg(&output);
        let result = run_with_timeout(&mut command);
        assert!(!result.status.success(), "stale template was selected");
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("no quickjs template covers these capabilities"),
            "unexpected error: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !output.exists(),
            "pack wrote an executable from a stale cache entry"
        );
    }

    #[test]
    fn cache_skips_same_version_runtime_without_template_api_marker() {
        let directory = TempDir::new("legacy-template-api");
        let cli = copy_cli(&directory);
        let entry = directory.path().join("main.js");
        let output = directory.path().join("packed-app");
        let cache = directory.path().join("cache");
        let stale = cache
            .join("templates")
            .join(env!("CARGO_PKG_VERSION"))
            .join("quickjs")
            .join("0000000000000000");
        fs::create_dir_all(&stale).expect("create same-version legacy cache entry");
        fs::write(&entry, "module.exports = 1;\n").expect("write app entry");
        let capabilities = all_capability_names();
        fake_template_with(
            &stale.join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
            "quickjs",
            &capabilities,
            false,
        );
        write_cached_capabilities(&stale, &capabilities);

        let result = run_pack(
            &cli,
            directory.path(),
            &entry,
            &["--output", output.to_str().expect("output path is UTF-8")],
        );
        assert!(!result.status.success(), "legacy runtime was selected");
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("no quickjs template covers these capabilities"),
            "unexpected error: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !output.exists(),
            "pack wrote an executable from a legacy runtime"
        );
    }

    #[test]
    fn cache_metadata_cannot_hide_missing_runtime_capabilities() {
        let directory = TempDir::new("cache-actual-capabilities");
        let cli = copy_cli(&directory);
        let entry = directory.path().join("main.js");
        let output = directory.path().join("packed-app");
        let cache = directory.path().join("cache");
        let cached = cache
            .join("templates")
            .join(env!("CARGO_PKG_VERSION"))
            .join("quickjs")
            .join("0000000000000000");
        fs::create_dir_all(&cached).expect("create cache entry");
        fs::write(&entry, "require('node:crypto');\n").expect("write crypto entry");

        let all = all_capability_names();
        let actual: Vec<_> = all
            .iter()
            .copied()
            .filter(|name| *name != "crypto")
            .collect();
        fake_template_with(
            &cached.join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
            "quickjs",
            &actual,
            true,
        );
        write_cached_capabilities(&cached, &all);

        let result = run_pack(
            &cli,
            directory.path(),
            &entry,
            &["--output", output.to_str().expect("output path is UTF-8")],
        );
        assert!(
            !result.status.success(),
            "insufficient cached runtime was selected"
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("no quickjs template covers these capabilities"),
            "{error}"
        );
        assert!(
            !output.exists(),
            "pack wrote an executable from an insufficient runtime"
        );
    }

    #[test]
    fn sibling_template_requires_current_version_and_capabilities() {
        for (name, version, missing_crypto) in [
            ("sibling-stale-version", "0.0.0", false),
            (
                "sibling-missing-capability",
                env!("CARGO_PKG_VERSION"),
                true,
            ),
        ] {
            let directory = TempDir::new(name);
            let cli = copy_cli(&directory);
            let entry = directory.path().join("main.js");
            let output = directory.path().join("packed-app");
            fs::write(&entry, "require('node:crypto');\n").expect("write crypto entry");

            let all = all_capability_names();
            let capabilities: Vec<_> = all
                .iter()
                .copied()
                .filter(|capability| !missing_crypto || *capability != "crypto")
                .collect();
            fake_template_with(
                &directory.path().join("thinpack-rt"),
                version,
                "quickjs",
                &capabilities,
                true,
            );

            let result = run_pack(
                &cli,
                directory.path(),
                &entry,
                &["--output", output.to_str().expect("output path is UTF-8")],
            );
            assert!(
                !result.status.success(),
                "invalid sibling template was selected"
            );
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(
                error.contains("no quickjs template covers these capabilities"),
                "{error}"
            );
            assert!(
                !output.exists(),
                "pack wrote an executable from an invalid sibling"
            );
        }
    }

    fn fake_template(path: &Path, version: &str) -> PathBuf {
        let capabilities = all_capability_names();
        fake_template_with(path, version, "quickjs", &capabilities, true)
    }

    fn fake_template_with(
        path: &Path,
        version: &str,
        backend: &str,
        capabilities: &[&str],
        template_api: bool,
    ) -> PathBuf {
        let mut module_output = String::new();
        for capability in capabilities {
            writeln!(module_output, "    printf 'module {capability}\\n'")
                .expect("writing to a String cannot fail");
        }
        let template_api = if template_api {
            "template-api 1\\n"
        } else {
            ""
        };
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n  --template-info)\n    printf 'version {version}\\nbackend {backend}\\n{template_api}capabilities-hash 0x0000000000000000\\n'\n    ;;\n  --capabilities)\n{module_output}    ;;\n  *) exit 2 ;;\nesac\n",
        );
        fs::write(path, script).expect("write fake runtime template");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make fake runtime template executable");
        path.to_path_buf()
    }

    fn all_capability_names() -> Vec<&'static str> {
        thinpack_caps::MODULES
            .iter()
            .map(|module| module.name)
            .collect()
    }

    fn write_cached_capabilities(directory: &Path, capabilities: &[&str]) {
        fs::write(
            directory.join("capabilities"),
            capabilities.join("\n") + "\n",
        )
        .expect("write cached capabilities");
    }

    fn run_pack(cli: &Path, directory: &Path, entry: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(cli);
        command
            .current_dir(directory)
            .env("THINPACK_CACHE", directory.join("cache"))
            .args(["pack"])
            .arg(entry)
            .args(args);
        run_with_timeout(&mut command)
    }

    fn run_cli(cli: &Path, directory: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(cli);
        command.current_dir(directory).args(args);
        run_with_timeout(&mut command)
    }

    fn copy_cli(directory: &TempDir) -> PathBuf {
        let source = Path::new(env!("CARGO_BIN_EXE_thinpack"));
        let destination = directory.path().join("thinpack");
        fs::copy(source, &destination).expect("copy thinpack CLI into isolated directory");
        fs::set_permissions(
            &destination,
            fs::metadata(source).expect("CLI metadata").permissions(),
        )
        .expect("preserve CLI executable permissions");
        destination
    }

    fn run_with_timeout(command: &mut Command) -> Output {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn thinpack CLI");
        let stdout = child.stdout.take().expect("capture CLI stdout");
        let stderr = child.stderr.take().expect("capture CLI stderr");
        let stdout = thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut stdout = stdout;
            stdout.read_to_end(&mut bytes).expect("read CLI stdout");
            bytes
        });
        let stderr = thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut stderr = stderr;
            stderr.read_to_end(&mut bytes).expect("read CLI stderr");
            bytes
        });

        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait for thinpack CLI") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let stdout = join_output(stdout, "stdout");
                let stderr = join_output(stderr, "stderr");
                panic!(
                    "thinpack CLI timed out after {} ms\nstdout: {}\nstderr: {}",
                    COMMAND_TIMEOUT.as_millis(),
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr),
                );
            }
            thread::sleep(Duration::from_millis(10));
        };

        Output {
            status,
            stdout: join_output(stdout, "stdout"),
            stderr: join_output(stderr, "stderr"),
        }
    }

    fn join_output(reader: thread::JoinHandle<Vec<u8>>, stream: &str) -> Vec<u8> {
        reader
            .join()
            .unwrap_or_else(|_| panic!("{stream} reader panicked"))
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "thinpack-cli-{name}-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create temporary test directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
