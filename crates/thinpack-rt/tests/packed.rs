use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use thinpack_pack::{Compression, FileRecord, Manifest, Template};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

struct PackedTest {
    name: &'static str,
    directory: &'static str,
    entry: &'static str,
    files: &'static [(&'static str, &'static str)],
    capabilities: &'static [&'static str],
}

const TESTS: &[PackedTest] = &[
    PackedTest {
        name: "packed::stat-mode-birthtime",
        directory: "stat-mode-birthtime",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const assert = require('node:assert/strict');
const fs = require('node:fs');

const directoryStats = fs.statSync('/app');
const fileStats = fs.statSync('/app/main.js');
assert.deepEqual(
  {
    directoryMode: directoryStats.mode,
    birthtimeValid:
      fileStats.birthtime instanceof Date &&
      !Number.isNaN(fileStats.birthtime.getTime()),
  },
  { directoryMode: 0o40755, birthtimeValid: true },
);
",
        )],
        capabilities: &["assert", "fs"],
    },
    PackedTest {
        name: "packed::error-paths",
        directory: "error-paths",
        entry: "/app/main.js",
        files: &[
            (
                "/app/main.js",
                r"
const assert = require('node:assert/strict');

// Resolver errors name the path the payload records, not a host rendering of it.
assert.throws(
  () => require('/app/native.node'),
  (error) => {
    assert.match(error.message, /\/app\/native\.node/);
    assert.doesNotMatch(error.message, /\\/);
    return true;
  },
);
",
            ),
            ("/app/native.node", ""),
        ],
        capabilities: &["assert"],
    },
    PackedTest {
        name: "packed::module-paths",
        directory: "module-paths",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const assert = require('node:assert/strict');

// Payload paths are POSIX paths on every platform, so the ids JS reports must
// not pick up the host separators of a Windows build.
assert.deepEqual(
  {
    dirname: __dirname,
    filename: module.filename,
    relative: require.resolve('./main.js'),
    absolute: require.resolve('/app/main.js'),
  },
  {
    dirname: '/app',
    filename: '/app/main.js',
    relative: '/app/main.js',
    absolute: '/app/main.js',
  },
);
",
        )],
        capabilities: &["assert"],
    },
    PackedTest {
        name: "packed::worker-vfs",
        directory: "worker-vfs",
        entry: "/app/main.js",
        files: &[
            (
                "/app/main.js",
                r"
const assert = require('node:assert/strict');
const { Worker } = require('node:worker_threads');

const worker = new Worker('/app/worker.js');
worker.once('error', (error) => {
  throw error;
});
worker.once('message', (message) => {
  assert.equal(message, 'worker loaded from the packed VFS');
});
worker.once('exit', (code) => {
  assert.equal(code, 0);
});
",
            ),
            (
                "/app/worker.js",
                r"
const { parentPort } = require('node:worker_threads');
const assert = require('node:assert/strict');

assert.equal(__dirname, '/app');
assert.equal(require.resolve('./worker.js'), '/app/worker.js');
parentPort.postMessage('worker loaded from the packed VFS');
",
            ),
        ],
        capabilities: &["assert", "worker_threads"],
    },
    PackedTest {
        name: "packed::zlib-constructors",
        directory: "zlib-constructors",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const assert = require('node:assert/strict');
const zlib = require('node:zlib');

// Dependencies such as crc32-stream subclass the named compression streams:
// they override `_transform`/`push`, call the inherited implementations, and
// run their own bookkeeping alongside the compressed output.
class DeflateCRC32Stream extends zlib.DeflateRaw {
  constructor(options) {
    super(options);
    this.checksum = 0;
    this.rawSize = 0;
    this.compressedSize = 0;
  }

  push(chunk, encoding) {
    if (chunk) this.compressedSize += chunk.length;
    return super.push(chunk, encoding);
  }

  _transform(chunk, encoding, callback) {
    this.checksum = zlib.crc32(chunk, this.checksum);
    this.rawSize += chunk.length;
    super._transform(chunk, encoding, callback);
  }
}

const input = Buffer.from('thinpack zlib constructor compatibility');
const stream = new DeflateCRC32Stream();
const chunks = [];
stream.on('data', (chunk) => chunks.push(chunk));
stream.on('end', () => {
  const compressed = Buffer.concat(chunks);
  assert.equal(stream.rawSize, input.length);
  assert.equal(stream.compressedSize, compressed.length);
  assert.equal(stream.checksum, zlib.crc32(input));
  assert.ok(zlib.inflateRawSync(compressed).equals(input));

  const pairs = [
    ['Gzip', 'createGzip'],
    ['Gunzip', 'createGunzip'],
    ['Deflate', 'createDeflate'],
    ['Inflate', 'createInflate'],
    ['DeflateRaw', 'createDeflateRaw'],
    ['InflateRaw', 'createInflateRaw'],
    ['Unzip', 'createUnzip'],
    ['BrotliCompress', 'createBrotliCompress'],
    ['BrotliDecompress', 'createBrotliDecompress'],
    ['ZstdCompress', 'createZstdCompress'],
    ['ZstdDecompress', 'createZstdDecompress'],
  ];
  for (const [name, factory] of pairs) {
    assert.equal(typeof zlib[name], 'function', name);
    assert.ok(zlib[factory]() instanceof zlib[name], factory);
  }
});
stream.end(input);
",
        )],
        capabilities: &["assert", "zlib"],
    },
    PackedTest {
        name: "packed::util-diff-without-assert",
        directory: "util-diff-without-assert",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
// `util.diff` uses the private myers_diff helper. The helper is registered by
// the always-linked internals extension, so a template selected with `util`
// alone can load it without pulling in the public `assert` capability.
const { diff } = require('node:util');

const check = (actual, expected) => {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`unexpected diff: ${JSON.stringify(actual)}`);
  }
};

check(diff('12345678', '12!!5!7!'), [
  [0, '1'],
  [0, '2'],
  [1, '3'],
  [1, '4'],
  [-1, '!'],
  [-1, '!'],
  [0, '5'],
  [1, '6'],
  [-1, '!'],
  [0, '7'],
  [1, '8'],
  [-1, '!'],
]);
check(diff(['1', '2', '3'], ['1', '3', '4']), [
  [0, '1'],
  [1, '2'],
  [0, '3'],
  [-1, '4'],
]);
check(diff('foo', 'foo'), []);
",
        )],
        capabilities: &["util"],
    },
];

pub(super) fn print_list(cli: &super::Cli) {
    for test in TESTS {
        if cli.matches(test.name) {
            println!("{}", test.name);
        }
    }
}

pub(super) fn execute(cli: &super::Cli) -> bool {
    if cli.ignored == super::Ignored::Only {
        return true;
    }

    let selected: Vec<&PackedTest> = TESTS.iter().filter(|test| cli.matches(test.name)).collect();
    if selected.is_empty() {
        return true;
    }

    println!(
        "running {} packed test{}",
        selected.len(),
        if selected.len() == 1 { "" } else { "s" }
    );
    let mut passed = 0;
    let mut failed = 0;
    for test in selected {
        let started = Instant::now();
        match run_test(test) {
            super::Outcome::Passed => {
                passed += 1;
                println!("ok {} ({:.2}s)", test.name, started.elapsed().as_secs_f64());
            }
            super::Outcome::Failed(report) => {
                failed += 1;
                println!(
                    "FAILED {} ({:.2}s)\n{report}",
                    test.name,
                    started.elapsed().as_secs_f64()
                );
            }
        }
    }
    println!("packed test result: {passed} passed; {failed} failed");
    failed == 0
}

fn run_test(test: &PackedTest) -> super::Outcome {
    let directory = match TempDir::new(test.directory) {
        Ok(directory) => directory,
        Err(error) => {
            return super::Outcome::Failed(format!("cannot create test directory: {error}"));
        }
    };
    let binary = match build_packed_binary(test, directory.path()) {
        Ok(binary) => binary,
        Err(error) => {
            return super::Outcome::Failed(format!("cannot build packed test executable: {error}"));
        }
    };
    run_binary(&binary, directory.path())
}

fn build_packed_binary(
    test: &PackedTest,
    directory: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let runtime = Path::new(env!("CARGO_BIN_EXE_thinpack-rt"));
    let template = inspect_template(runtime)?;
    let mut blob = Vec::new();
    let mut files = Vec::with_capacity(test.files.len());
    for (path, source) in test.files {
        let offset = u64::try_from(blob.len())?;
        let contents = source.as_bytes();
        let length = u32::try_from(contents.len())?;
        files.push(FileRecord {
            path: (*path).to_owned(),
            offset,
            length,
        });
        blob.extend_from_slice(contents);
    }

    let capabilities = capabilities(test.capabilities);
    let manifest = Manifest {
        entry: test.entry.to_owned(),
        capabilities,
        template,
        files,
        assets: Vec::new(),
    };
    let payload = thinpack_pack::assemble(&manifest, &blob, Compression::None)?;
    let runtime_bytes = fs::read(runtime)?;
    let executable = thinpack_pack::append_to_binary(&runtime_bytes, &payload)?;
    #[cfg(windows)]
    let name = "packed-test.exe";
    #[cfg(not(windows))]
    let name = "packed-test";
    let path = directory.join(name);
    fs::write(&path, executable)?;
    fs::set_permissions(&path, fs::metadata(runtime)?.permissions())?;
    Ok(path)
}

fn inspect_template(runtime: &Path) -> Result<Template, Box<dyn std::error::Error>> {
    let output = Command::new(runtime).arg("--template-info").output()?;
    if !output.status.success() {
        return Err(format!(
            "{} --template-info failed: {}",
            runtime.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }

    let output = String::from_utf8(output.stdout)?;
    let mut version = None;
    let mut backend = None;
    let mut capabilities_hash = None;
    for line in output.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "version" => version = Some(value.to_owned()),
            "backend" => backend = Some(value.to_owned()),
            "capabilities-hash" => {
                capabilities_hash = Some(u64::from_str_radix(value.trim_start_matches("0x"), 16)?);
            }
            _ => {}
        }
    }

    Ok(Template {
        version: version.ok_or("runtime reported no version")?,
        backend: backend.ok_or("runtime reported no backend")?,
        capabilities_hash: capabilities_hash.ok_or("runtime reported no capabilities hash")?,
    })
}

fn capabilities(roots: &[&str]) -> Vec<String> {
    let mut names: BTreeSet<String> = thinpack_caps::baseline()
        .map(|module| module.name.to_owned())
        .collect();
    let roots: Vec<String> = roots.iter().map(|name| (*name).to_owned()).collect();
    names.extend(
        thinpack_caps::closure(&roots)
            .expect("packed test roots are known capabilities")
            .into_iter()
            .map(|module| module.name.to_owned()),
    );
    names.into_iter().collect()
}

fn run_binary(binary: &Path, directory: &Path) -> super::Outcome {
    let mut command = Command::new(binary);
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    super::configure_process_group(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return super::Outcome::Failed(format!("cannot spawn packed executable: {error}"));
        }
    };
    let pid = child.id();
    let Some(stdout) = child.stdout.take() else {
        return super::Outcome::Failed("packed executable stdout is not piped".to_owned());
    };
    let Some(stderr) = child.stderr.take() else {
        return super::Outcome::Failed("packed executable stderr is not piped".to_owned());
    };
    let stdout = thread::spawn(move || super::read_all(stdout));
    let stderr = thread::spawn(move || super::read_all(stderr));

    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                return super::Outcome::Failed(format!(
                    "cannot wait for packed executable: {error}"
                ));
            }
        }
        if Instant::now() >= deadline {
            timed_out = true;
            super::kill_process_group(pid);
            if let Err(error) = child.kill() {
                return super::Outcome::Failed(format!(
                    "cannot kill timed-out packed test: {error}"
                ));
            }
            match child.wait() {
                Ok(status) => break status,
                Err(error) => {
                    return super::Outcome::Failed(format!(
                        "cannot reap timed-out packed test: {error}"
                    ));
                }
            }
        }
        thread::sleep(Duration::from_millis(25));
    };

    super::kill_process_group(pid);
    let stdout = super::read(stdout);
    let stderr = super::read(stderr);
    if status.success() && !timed_out {
        return super::Outcome::Passed;
    }

    let reason = if timed_out {
        format!("timed out after {} ms", TEST_TIMEOUT.as_millis())
    } else {
        format!("exited with {}", super::describe_status(status))
    };
    let mut report = format!("{reason}\n");
    if !stdout.is_empty() {
        report.push_str("\n----- stdout -----\n");
        report.push_str(&super::truncate(&stdout));
    }
    if !stderr.is_empty() {
        report.push_str("\n----- stderr -----\n");
        report.push_str(&super::truncate(&stderr));
    }
    super::Outcome::Failed(report)
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "thinpack-packed-{name}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
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
