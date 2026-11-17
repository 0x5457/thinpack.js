use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;

pub(crate) const OPS: &[OpDecl] = &[
    op_process_argv(),
    op_process_env(),
    op_process_cwd(),
    op_process_chdir(),
    op_process_info(),
    op_process_exit(),
    op_process_kill(),
    op_process_umask(),
    op_process_ids(),
    op_process_memory(),
    op_process_cpu_usage(),
    op_process_resource_usage(),
    op_process_tty_size(),
    op_process_set_raw_mode(),
    op_process_is_raw(),
];

#[op2]
#[string]
pub(crate) fn op_process_argv() -> String {
    let mut arguments = std::env::args_os().map(|arg| arg.to_string_lossy().into_owned());
    let executable = arguments.next().unwrap_or_default();
    let rest: Vec<String> = arguments.collect();

    let argv: Vec<String> = if matches!(rest.first().map(String::as_str), Some("-e" | "--eval")) {
        std::iter::once(executable)
            .chain(rest.into_iter().skip(2))
            .collect()
    } else {
        std::iter::once(executable).chain(rest).collect()
    };
    argv.join("\0")
}

#[op2]
#[string]
pub(crate) fn op_process_env() -> String {
    let mut entries: Vec<String> = std::env::vars_os()
        .map(|(key, value)| format!("{}={}", key.to_string_lossy(), value.to_string_lossy()))
        .collect();
    entries.sort();
    entries.join("\0")
}

#[op2]
#[string]
pub(crate) fn op_process_cwd() -> String {
    std::env::current_dir()
        .map(|directory| directory.display().to_string())
        .unwrap_or_default()
}

#[op2(nofast)]
pub(crate) fn op_process_chdir(#[string] path: &str) -> Result<(), JsErrorBox> {
    std::env::set_current_dir(path).map_err(|error| {
        let errno = error.raw_os_error().map_or(5, |errno| -errno);
        JsErrorBox::new("Error", errno.to_string())
    })
}

#[op2]
#[string]
pub(crate) fn op_process_info() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    // Node calls macOS "darwin"; `std::env::consts::OS` calls it "macos".
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{platform}\0{arch}\0{}", std::process::id())
}

#[op2(fast)]
pub(crate) fn op_process_exit(code: i32) {
    restore_saved_terminals();
    std::process::exit(code);
}

#[op2(fast)]
pub(crate) fn op_process_kill(pid: i32, signal: i32) -> Result<(), JsErrorBox> {
    // SAFETY: `kill(2)` takes plain integers and reports a bad pid or signal through its
    // return value instead of being undefined behaviour.
    if unsafe { libc::kill(pid, signal) } == -1 {
        let error = std::io::Error::last_os_error();
        let errno = error.raw_os_error().unwrap_or(libc::EINVAL);
        return Err(JsErrorBox::new("Error", format!("-{errno}\0{error}")));
    }
    Ok(())
}

#[cfg(target_vendor = "apple")]
#[expect(
    clippy::cast_possible_truncation,
    reason = "macOS mode_t is u16; umask only reads the low permission bits"
)]
const fn umask_mode(mask: i32) -> libc::mode_t {
    mask.cast_unsigned() as libc::mode_t
}

#[cfg(not(target_vendor = "apple"))]
const fn umask_mode(mask: i32) -> libc::mode_t {
    mask.cast_unsigned()
}

#[cfg(target_vendor = "apple")]
fn umask_value(mode: libc::mode_t) -> i32 {
    i32::from(mode)
}

#[cfg(not(target_vendor = "apple"))]
const fn umask_value(mode: libc::mode_t) -> i32 {
    mode.cast_signed()
}

#[op2(fast)]
pub(crate) fn op_process_umask(#[smi] mask: i32) -> i32 {
    // SAFETY: `umask` only reads and replaces the process umask; every bit pattern is valid.
    let previous = unsafe { libc::umask(0) };
    if mask >= 0 {
        // SAFETY: `umask` only reads and replaces the process umask; every bit pattern is valid.
        unsafe { libc::umask(umask_mode(mask)) };
    } else {
        // SAFETY: `umask` only reads and replaces the process umask; every bit pattern is valid.
        unsafe { libc::umask(previous) };
    }
    umask_value(previous)
}

#[op2]
#[string]
pub(crate) fn op_process_ids() -> String {
    format!(
        "{}\0{}\0{}\0{}",
        // SAFETY: `getuid` takes no arguments and cannot fail.
        unsafe { libc::getuid() },
        // SAFETY: `getgid` takes no arguments and cannot fail.
        unsafe { libc::getgid() },
        // SAFETY: `geteuid` takes no arguments and cannot fail.
        unsafe { libc::geteuid() },
        // SAFETY: `getegid` takes no arguments and cannot fail.
        unsafe { libc::getegid() }
    )
}

#[op2]
#[string]
pub(crate) fn op_process_memory() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let rss_kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    format!("{}\0\0\0\0\0", rss_kib * 1024)
}

fn rusage() -> Result<libc::rusage, JsErrorBox> {
    // SAFETY: `rusage` is a plain-data struct for which all-zero is a valid value.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `getrusage` writes into the `rusage` supplied through the pointer.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) } != 0 {
        let errno = std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO);
        return Err(JsErrorBox::new("Error", format!("-{errno}")));
    }
    Ok(usage)
}

/// Converts a `timeval` to microseconds.
///
/// `tv_usec` is `suseconds_t`, which is `i64` on Linux and `i32` on Apple platforms.
const fn micros(time: libc::timeval) -> i64 {
    #[cfg(target_vendor = "apple")]
    let usec = time.tv_usec as i64;
    #[cfg(not(target_vendor = "apple"))]
    let usec = time.tv_usec;
    time.tv_sec * 1_000_000 + usec
}

#[op2]
#[string]
pub(crate) fn op_process_cpu_usage() -> Result<String, JsErrorBox> {
    let usage = rusage()?;
    Ok(format!(
        "{}\0{}",
        micros(usage.ru_utime),
        micros(usage.ru_stime)
    ))
}

#[op2]
#[string]
pub(crate) fn op_process_resource_usage() -> Result<String, JsErrorBox> {
    let usage = rusage()?;
    Ok(format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        micros(usage.ru_utime),
        micros(usage.ru_stime),
        usage.ru_maxrss,
        usage.ru_ixrss,
        usage.ru_idrss,
        usage.ru_isrss,
        usage.ru_minflt,
        usage.ru_majflt,
        usage.ru_nswap,
        usage.ru_inblock,
        usage.ru_oublock,
        usage.ru_msgsnd,
        usage.ru_msgrcv,
        usage.ru_nsignals,
        usage.ru_nvcsw,
        usage.ru_nivcsw,
    ))
}

#[op2]
#[string]
pub(crate) fn op_process_tty_size(#[smi] fd: i32) -> String {
    // SAFETY: `winsize` is a plain-data struct for which all-zero is a valid value.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: `TIOCGWINSZ` writes a `winsize` through the pointer argument.
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut size) } == 0 {
        format!("{}\0{}", size.ws_col, size.ws_row)
    } else {
        format!("{}\0{}", 0, 0)
    }
}

static SAVED_TERMIOS: LazyLock<Mutex<HashMap<i32, libc::termios>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const fn apply_raw(settings: &mut libc::termios) {
    settings.c_iflag &= !(libc::IGNBRK
        | libc::BRKINT
        | libc::PARMRK
        | libc::ISTRIP
        | libc::INLCR
        | libc::IGNCR
        | libc::ICRNL
        | libc::IXON);
    settings.c_oflag &= !libc::OPOST;
    settings.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
    settings.c_cflag &= !(libc::CSIZE | libc::PARENB);
    settings.c_cflag |= libc::CS8;
    settings.c_cc[libc::VMIN] = 1;
    settings.c_cc[libc::VTIME] = 0;
}

fn get_termios(fd: i32) -> Result<libc::termios, i32> {
    // SAFETY: `termios` is a plain-data struct for which all-zero is a valid value.
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: `tcgetattr` writes a `termios` through the pointer argument.
    if unsafe { libc::tcgetattr(fd, &raw mut settings) } == 0 {
        Ok(settings)
    } else {
        Err(errno())
    }
}

fn set_termios(fd: i32, settings: &libc::termios) -> Result<(), i32> {
    // SAFETY: `tcsetattr` only reads the `termios` supplied through the pointer argument.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, settings) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EINVAL)
}

#[op2(nofast)]
pub(crate) fn op_process_set_raw_mode(#[smi] fd: i32, enable: bool) -> Result<(), JsErrorBox> {
    let failure = |errno: i32| JsErrorBox::new("Error", (-errno).to_string());

    if !enable {
        let saved = SAVED_TERMIOS.lock().unwrap().remove(&fd);
        return saved.map_or_else(
            || Ok(()),
            |settings| set_termios(fd, &settings).map_err(failure),
        );
    }

    let current = get_termios(fd).map_err(failure)?;
    let mut raw = current;
    apply_raw(&mut raw);
    set_termios(fd, &raw).map_err(failure)?;
    SAVED_TERMIOS.lock().unwrap().entry(fd).or_insert(current);
    Ok(())
}

#[op2(fast)]
pub(crate) fn op_process_is_raw(#[smi] fd: i32) -> bool {
    SAVED_TERMIOS.lock().unwrap().contains_key(&fd)
}

fn restore_saved_terminals() {
    let saved = std::mem::take(&mut *SAVED_TERMIOS.lock().unwrap());
    for (fd, settings) in saved {
        let _ = set_termios(fd, &settings);
    }
}
