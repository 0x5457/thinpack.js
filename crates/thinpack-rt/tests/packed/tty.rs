//! Process terminals over a real pseudo-terminal.
//!
//! `PackedTest` attaches pipes, which `isTTY` reports as not terminals, so the
//! window size, raw mode and its restoration need a pty. Windows consoles have
//! no equivalent harness here; the same `uv_compat` calls run there.

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::super::{
    Cli, Outcome, configure_process_group, describe_status, kill_process_group, truncate,
};
use super::{TEST_TIMEOUT, TempDir, build_packed_payload};

/// The window size the pty reports, chosen so it cannot be a default.
const COLUMNS: u16 = 100;
const ROWS: u16 = 30;

/// Printed while the terminal is in raw mode, so the harness can inspect it.
const RAW_MARKER: &str = "raw-on";

/// Checked inside the program before the epilogue runs.
const SETUP: &str = r"
const assert = require('node:assert/strict');

assert.equal(process.stdout.isTTY, true);
assert.equal(process.stdin.isTTY, true);
assert.deepEqual([process.stdout.columns, process.stdout.rows], [100, 30]);
assert.deepEqual(process.stdout.getWindowSize(), [100, 30]);
assert.equal(process.stderr.columns, 100);
assert.equal(process.stdin.isRaw, false);

// The color methods are shared with tty.WriteStream.prototype and load on use;
// a packed runtime must not require them during extension startup.
assert.equal(process.stdout.getColorDepth({ FORCE_COLOR: '3' }), 24);
assert.equal(process.stdout.hasColors(256, { FORCE_COLOR: '2' }), true);

process.stdin.setRawMode(true);
assert.equal(process.stdin.isRaw, true);
console.log('raw-on');
";

pub(super) struct TtyTest {
    pub(super) name: &'static str,
    directory: &'static str,
    /// Runs with the terminal still in raw mode and must leave it restored.
    epilogue: &'static str,
}

const TESTS: &[TtyTest] = &[
    TtyTest {
        name: "packed::tty-raw-mode-restored-on-exit",
        directory: "tty-raw-mode-exit",
        // The delay only gives the harness time to observe the raw terminal
        // before the process ends on its own.
        epilogue: "setTimeout(() => {}, 400);\n",
    },
    TtyTest {
        name: "packed::tty-raw-mode-restored-on-process-exit",
        directory: "tty-raw-mode-process-exit",
        // The delay only gives the harness time to observe the raw terminal
        // before the process leaves through the exit op.
        epilogue: "setTimeout(() => process.exit(0), 500);\n",
    },
];

pub(super) fn selected(cli: &Cli) -> Vec<&'static TtyTest> {
    TESTS.iter().filter(|test| cli.matches(test.name)).collect()
}

pub(super) fn print_list(cli: &Cli) {
    for test in selected(cli) {
        println!("{}", test.name);
    }
}

pub(super) fn run_test(test: &TtyTest) -> Outcome {
    match attempt(test) {
        Ok(()) => Outcome::Passed,
        Err(report) => Outcome::Failed(report),
    }
}

fn attempt(test: &TtyTest) -> Result<(), String> {
    let directory = TempDir::new(test.directory)
        .map_err(|error| format!("cannot create test directory: {error}"))?;
    let source = format!("{SETUP}{}", test.epilogue);
    let files = [("/app/main.js", source.as_str())];
    let binary = build_packed_payload("/app/main.js", &files, &[], directory.path())
        .map_err(|error| format!("cannot build packed test executable: {error}"))?;

    let (master, slave, inspect) = open_terminal()?;
    let mut child = spawn(&binary, directory.path(), &slave)?;

    let mut output = Vec::new();
    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut inspected = false;
    let status = loop {
        read_available(&master, &mut output)?;
        if !inspected
            && output
                .windows(RAW_MARKER.len())
                .any(|w| w == RAW_MARKER.as_bytes())
        {
            inspected = true;
            require_raw(inspect.as_raw_fd())?;
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for packed executable: {error}"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            kill(&mut child);
            return Err(format!(
                "timed out after {} ms\n{}",
                TEST_TIMEOUT.as_millis(),
                describe(&output, None)
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    read_available(&master, &mut output)?;

    if !status.success() {
        return Err(describe(&output, Some(status)));
    }
    if !inspected {
        return Err(format!(
            "the program never reported raw mode\n{}",
            describe(&output, None)
        ));
    }
    // The program left in raw mode, so canonical, echoing mode means the exit
    // path restored the terminal.
    let (echo, canonical) = line_flags(inspect.as_raw_fd())?;
    if !echo || !canonical {
        return Err(format!(
            "the terminal was left in raw mode (echo {echo}, canonical {canonical})\n{}",
            describe(&output, Some(status))
        ));
    }
    Ok(())
}

fn kill(child: &mut Child) {
    kill_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

fn describe(output: &[u8], status: Option<ExitStatus>) -> String {
    let mut report = status.map_or_else(String::new, |status| {
        format!("exited with {}\n", describe_status(status))
    });
    if !output.is_empty() {
        report.push_str("----- output -----\n");
        report.push_str(&truncate(&String::from_utf8_lossy(output)));
    }
    report
}

/// Open a pty of the size the tests expect: the master for the harness, the
/// slave path for the child's stdio, and a parent-side slave for inspecting
/// the terminal before and after the child runs.
fn open_terminal() -> Result<(OwnedFd, CString, OwnedFd), String> {
    // SAFETY: `posix_openpt` only reads its flags.
    let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
    if master < 0 {
        return Err(format!("cannot open a pty: {}", io::Error::last_os_error()));
    }
    // SAFETY: `master` is the descriptor just opened, which nothing else owns.
    let master = unsafe { OwnedFd::from_raw_fd(master) };

    // SAFETY: the master descriptor is valid for both calls.
    let prepared = unsafe {
        libc::grantpt(master.as_raw_fd()) == 0 && libc::unlockpt(master.as_raw_fd()) == 0
    };
    if !prepared {
        return Err(format!(
            "cannot prepare the pty: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: `ptsname` returns a pointer to a static buffer owned by libc.
    let slave = unsafe { libc::ptsname(master.as_raw_fd()) };
    if slave.is_null() {
        return Err(format!(
            "cannot name the pty: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: the pointer above is NUL-terminated.
    let slave = unsafe { CStr::from_ptr(slave) }.to_owned();
    let inspect = open_slave(&slave)?;

    // The window size belongs to the terminal, and only the slave side accepts
    // TIOCSWINSZ on macOS.
    let winsize = libc::winsize {
        ws_row: ROWS,
        ws_col: COLUMNS,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `TIOCSWINSZ` only reads the `winsize` argument.
    if unsafe { libc::ioctl(inspect.as_raw_fd(), libc::TIOCSWINSZ, &winsize) } != 0 {
        return Err(format!(
            "cannot size the pty: {}",
            io::Error::last_os_error()
        ));
    }

    // SAFETY: the master descriptor is valid for both calls.
    let configured = unsafe {
        let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
        flags >= 0 && libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
    };
    if !configured {
        return Err(format!(
            "cannot make the pty non-blocking: {}",
            io::Error::last_os_error()
        ));
    }

    Ok((master, slave, inspect))
}

fn open_slave(path: &CStr) -> Result<OwnedFd, String> {
    // SAFETY: `path` is NUL-terminated.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };
    if fd < 0 {
        return Err(format!(
            "cannot open the pty slave: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: `fd` is the descriptor just opened, which nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn spawn(binary: &Path, directory: &Path, slave: &CStr) -> Result<Child, String> {
    let mut command = Command::new(binary);
    command
        .current_dir(directory)
        .stdin(Stdio::from(open_slave(slave)?))
        .stdout(Stdio::from(open_slave(slave)?))
        .stderr(Stdio::from(open_slave(slave)?));
    configure_process_group(&mut command);
    command
        .spawn()
        .map_err(|error| format!("cannot spawn packed executable: {error}"))
}

fn read_available(master: &OwnedFd, output: &mut Vec<u8>) -> Result<(), String> {
    let mut buffer = [0u8; 4096];
    loop {
        // SAFETY: `buffer` is valid for the length passed.
        let read =
            unsafe { libc::read(master.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if read > 0 {
            output.extend_from_slice(&buffer[..read.cast_unsigned()]);
            continue;
        }
        if read == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EIO) {
            // Every slave is closed; the output read so far is complete.
            return Ok(());
        }
        if matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ) {
            return Ok(());
        }
        return Err(format!("cannot read the pty: {error}"));
    }
}

/// `(echo, canonical)` for the terminal behind `fd`.
fn line_flags(fd: i32) -> Result<(bool, bool), String> {
    // SAFETY: all-zero is a valid `termios`.
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: `tcgetattr` writes a `termios` through the pointer argument.
    if unsafe { libc::tcgetattr(fd, &raw mut settings) } != 0 {
        return Err(format!(
            "cannot read the terminal mode: {}",
            io::Error::last_os_error()
        ));
    }
    Ok((
        settings.c_lflag & libc::ECHO != 0,
        settings.c_lflag & libc::ICANON != 0,
    ))
}

fn require_raw(fd: i32) -> Result<(), String> {
    let (echo, canonical) = line_flags(fd)?;
    if echo || canonical {
        return Err(format!(
            "raw mode left the terminal canonical and echoing (echo {echo}, canonical {canonical})"
        ));
    }
    Ok(())
}
