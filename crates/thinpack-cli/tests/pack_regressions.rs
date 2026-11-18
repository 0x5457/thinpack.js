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

    fn fake_template(path: &Path, version: &str) -> PathBuf {
        let mut module_output = String::new();
        for module in thinpack_caps::MODULES {
            writeln!(module_output, "    printf 'module {}\\n'", module.name)
                .expect("writing to a String cannot fail");
        }
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n  --template-info)\n    printf 'version {version}\\nbackend quickjs\\ncapabilities-hash 0x0000000000000000\\n'\n    ;;\n  --capabilities)\n{module_output}    ;;\n  *) exit 2 ;;\nesac\n",
        );
        fs::write(path, script).expect("write fake runtime template");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make fake runtime template executable");
        path.to_path_buf()
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
