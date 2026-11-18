#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
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
    // Node's names differ from `std::env::consts::OS` for two targets.
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    format!("{platform}\0{arch}\0{}", std::process::id())
}

#[op2(fast)]
pub(crate) fn op_process_exit(code: i32) {
    restore_saved_terminals();
    std::process::exit(code);
}

#[cfg(unix)]
fn kill(pid: i32, signal: i32) -> Result<(), JsErrorBox> {
    // SAFETY: `kill(2)` takes plain integers and reports a bad pid or signal through its
    // return value instead of being undefined behaviour.
    if unsafe { libc::kill(pid, signal) } == -1 {
        let error = std::io::Error::last_os_error();
        let errno = error.raw_os_error().unwrap_or(libc::EINVAL);
        return Err(JsErrorBox::new("Error", format!("-{errno}\0{error}")));
    }
    Ok(())
}

#[cfg(windows)]
fn kill(pid: i32, signal: i32) -> Result<(), JsErrorBox> {
    win32::kill(pid, signal)
}

#[op2(fast)]
pub(crate) fn op_process_kill(pid: i32, signal: i32) -> Result<(), JsErrorBox> {
    kill(pid, signal)
}

#[cfg(unix)]
#[cfg(target_vendor = "apple")]
#[expect(
    clippy::cast_possible_truncation,
    reason = "macOS mode_t is u16; umask only reads the low permission bits"
)]
const fn umask_mode(mask: i32) -> libc::mode_t {
    mask.cast_unsigned() as libc::mode_t
}

#[cfg(unix)]
#[cfg(not(target_vendor = "apple"))]
const fn umask_mode(mask: i32) -> libc::mode_t {
    mask.cast_unsigned()
}

#[cfg(unix)]
#[cfg(target_vendor = "apple")]
fn umask_value(mode: libc::mode_t) -> i32 {
    i32::from(mode)
}

#[cfg(unix)]
#[cfg(not(target_vendor = "apple"))]
const fn umask_value(mode: libc::mode_t) -> i32 {
    mode.cast_signed()
}

#[cfg(unix)]
fn umask(mask: i32) -> i32 {
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

#[cfg(windows)]
fn umask(mask: i32) -> i32 {
    // The Windows CRT keeps the same file mode mask, as Deno's std_fs uses.
    unsafe extern "C" {
        fn _umask(mask: std::ffi::c_int) -> std::ffi::c_int;
    }
    // SAFETY: `_umask` sets the process mask and returns the previous value.
    unsafe {
        if mask >= 0 {
            _umask(mask)
        } else {
            let previous = _umask(0);
            _umask(previous);
            previous
        }
    }
}

#[op2(fast)]
pub(crate) fn op_process_umask(#[smi] mask: i32) -> i32 {
    umask(mask)
}

#[cfg(unix)]
fn ids() -> String {
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

#[cfg(windows)]
fn ids() -> String {
    // Node does not expose uid/gid on Windows; the fields stay empty.
    format!("{}\0{}\0{}\0{}", 0, 0, 0, 0)
}

#[op2]
#[string]
pub(crate) fn op_process_ids() -> String {
    ids()
}

#[cfg(unix)]
fn memory() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let rss_kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    format!("{}\0\0\0\0\0", rss_kib * 1024)
}

#[cfg(windows)]
fn memory() -> String {
    win32::memory()
}

#[op2]
#[string]
pub(crate) fn op_process_memory() -> String {
    memory()
}

#[cfg(unix)]
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
#[cfg(unix)]
const fn micros(time: libc::timeval) -> i64 {
    #[cfg(target_vendor = "apple")]
    let usec = time.tv_usec as i64;
    #[cfg(not(target_vendor = "apple"))]
    let usec = time.tv_usec;
    time.tv_sec * 1_000_000 + usec
}

#[cfg(unix)]
fn cpu_usage() -> Result<String, JsErrorBox> {
    let usage = rusage()?;
    Ok(format!(
        "{}\0{}",
        micros(usage.ru_utime),
        micros(usage.ru_stime)
    ))
}

#[cfg(windows)]
fn cpu_usage() -> Result<String, JsErrorBox> {
    win32::cpu_usage()
}

#[cfg(unix)]
fn resource_usage() -> Result<String, JsErrorBox> {
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

#[cfg(windows)]
fn resource_usage() -> Result<String, JsErrorBox> {
    win32::resource_usage()
}

#[op2]
#[string]
pub(crate) fn op_process_cpu_usage() -> Result<String, JsErrorBox> {
    cpu_usage()
}

#[op2]
#[string]
pub(crate) fn op_process_resource_usage() -> Result<String, JsErrorBox> {
    resource_usage()
}

#[cfg(unix)]
fn tty_size(fd: i32) -> String {
    // SAFETY: `winsize` is a plain-data struct for which all-zero is a valid value.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: `TIOCGWINSZ` writes a `winsize` through the pointer argument.
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut size) } == 0 {
        format!("{}\0{}", size.ws_col, size.ws_row)
    } else {
        format!("{}\0{}", 0, 0)
    }
}

#[cfg(windows)]
fn tty_size(fd: i32) -> String {
    win32::tty_size(fd)
}

#[op2]
#[string]
pub(crate) fn op_process_tty_size(#[smi] fd: i32) -> String {
    tty_size(fd)
}

#[cfg(unix)]
static SAVED_TERMIOS: LazyLock<Mutex<HashMap<i32, libc::termios>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
fn set_termios(fd: i32, settings: &libc::termios) -> Result<(), i32> {
    // SAFETY: `tcsetattr` only reads the `termios` supplied through the pointer argument.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, settings) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

#[cfg(unix)]
fn errno() -> i32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EINVAL)
}

#[cfg(unix)]
fn set_raw_mode(fd: i32, enable: bool) -> Result<(), JsErrorBox> {
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

#[cfg(windows)]
fn set_raw_mode(fd: i32, enable: bool) -> Result<(), JsErrorBox> {
    win32::set_raw_mode(fd, enable)
}

#[op2(nofast)]
pub(crate) fn op_process_set_raw_mode(#[smi] fd: i32, enable: bool) -> Result<(), JsErrorBox> {
    set_raw_mode(fd, enable)
}

#[cfg(unix)]
fn is_raw(fd: i32) -> bool {
    SAVED_TERMIOS.lock().unwrap().contains_key(&fd)
}

#[cfg(windows)]
fn is_raw(fd: i32) -> bool {
    win32::is_raw(fd)
}

#[op2(fast)]
pub(crate) fn op_process_is_raw(#[smi] fd: i32) -> bool {
    is_raw(fd)
}

#[cfg(unix)]
fn restore_saved_terminals() {
    let saved = std::mem::take(&mut *SAVED_TERMIOS.lock().unwrap());
    for (fd, settings) in saved {
        let _ = set_termios(fd, &settings);
    }
}

#[cfg(windows)]
fn restore_saved_terminals() {
    win32::restore_terminals();
}

/// Deno's Windows process layer: console modes, process times and memory
/// counters through the same `windows-sys` APIs `ext/node/ops/process.rs` uses.
#[cfg(windows)]
mod win32 {
    use std::collections::HashMap;
    use std::mem::{MaybeUninit, size_of};
    use std::sync::{LazyLock, Mutex};

    use deno_error::JsErrorBox;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, ENABLE_WINDOW_INPUT, GetConsoleMode,
        GetConsoleScreenBufferInfo, SetConsoleMode,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE, TerminateProcess,
    };

    static SAVED_MODES: LazyLock<Mutex<HashMap<i32, u32>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn console_handle(fd: i32) -> Option<HANDLE> {
        // SAFETY: `_get_osfhandle` maps a CRT descriptor to its OS handle and reports an
        // invalid descriptor with -1.
        let raw = unsafe { libc::get_osfhandle(fd) };
        (raw != -1).then_some(raw as HANDLE)
    }

    fn io_error() -> JsErrorBox {
        let error = std::io::Error::last_os_error();
        let errno = error.raw_os_error().unwrap_or(libc::EINVAL);
        JsErrorBox::new("Error", format!("-{errno}\0{error}"))
    }

    pub(super) fn kill(pid: i32, signal: i32) -> Result<(), JsErrorBox> {
        let access = if signal == 0 {
            PROCESS_QUERY_LIMITED_INFORMATION
        } else {
            PROCESS_TERMINATE
        };
        // SAFETY: plain Win32 call; the returned handle is closed before returning.
        let process = unsafe { OpenProcess(access, 0, pid.cast_unsigned()) };
        if process.is_null() {
            return Err(io_error());
        }
        let result = if signal == 0 {
            Ok(())
        // SAFETY: `process` is the handle opened above and is still valid.
        } else if unsafe { TerminateProcess(process, 1) } == 0 {
            Err(io_error())
        } else {
            Ok(())
        };
        // SAFETY: `process` is a valid handle opened by `OpenProcess`.
        unsafe { CloseHandle(process) };
        result
    }

    fn memory_counters() -> Option<PROCESS_MEMORY_COUNTERS> {
        let mut counters = MaybeUninit::<PROCESS_MEMORY_COUNTERS>::uninit();
        // SAFETY: Win32 call with a correctly sized output buffer.
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                counters.as_mut_ptr(),
                size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            )
        };
        if ok == 0 {
            None
        } else {
            // SAFETY: the call succeeded, so the buffer is initialized.
            Some(unsafe { counters.assume_init() })
        }
    }

    pub(super) fn memory() -> String {
        let rss = memory_counters().map_or(0, |counters| counters.WorkingSetSize);
        format!("{rss}\0\0\0\0\0")
    }

    /// User and kernel CPU time in microseconds, from `GetProcessTimes`.
    fn cpu_times() -> (u64, u64) {
        let mut creation = MaybeUninit::<FILETIME>::zeroed();
        let mut exit = MaybeUninit::<FILETIME>::zeroed();
        let mut kernel = MaybeUninit::<FILETIME>::zeroed();
        let mut user = MaybeUninit::<FILETIME>::zeroed();
        // SAFETY: Win32 call with valid out-pointers.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                creation.as_mut_ptr(),
                exit.as_mut_ptr(),
                kernel.as_mut_ptr(),
                user.as_mut_ptr(),
            )
        };
        if ok == 0 {
            return (0, 0);
        }
        // SAFETY: the call succeeded, so every out-pointer is initialized.
        let (kernel, user) = unsafe { (kernel.assume_init(), user.assume_init()) };
        let ticks =
            |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        // FILETIME counts 100 ns intervals; `process.cpuUsage()` reports microseconds.
        (ticks(user) / 10, ticks(kernel) / 10)
    }

    pub(super) fn cpu_usage() -> Result<String, JsErrorBox> {
        let (user, system) = cpu_times();
        Ok(format!("{user}\0{system}"))
    }

    pub(super) fn resource_usage() -> Result<String, JsErrorBox> {
        let (user, system) = cpu_times();
        let counters = memory_counters();
        let max_rss = counters.map_or(0, |counters| counters.PeakWorkingSetSize / 1024);
        let major_faults = counters.map_or(0, |counters| counters.PageFaultCount);
        let mut values = [0u64; 16];
        values[0] = user;
        values[1] = system;
        values[2] = max_rss as u64;
        values[7] = u64::from(major_faults);
        let text: Vec<String> = values.iter().map(u64::to_string).collect();
        Ok(text.join("\0"))
    }

    pub(super) fn tty_size(fd: i32) -> String {
        let Some(console) = console_handle(fd) else {
            return format!("{}\0{}", 0, 0);
        };
        let mut info = MaybeUninit::<CONSOLE_SCREEN_BUFFER_INFO>::zeroed();
        // SAFETY: valid console handle and out-pointer.
        if unsafe { GetConsoleScreenBufferInfo(console, info.as_mut_ptr()) } == 0 {
            return format!("{}\0{}", 0, 0);
        }
        // SAFETY: the call succeeded, so `info` is initialized.
        let info = unsafe { info.assume_init() };
        let columns = info.srWindow.Right - info.srWindow.Left + 1;
        let rows = info.srWindow.Bottom - info.srWindow.Top + 1;
        format!("{columns}\0{rows}")
    }

    fn set_console_mode(console: HANDLE, mode: u32) -> Result<(), JsErrorBox> {
        // SAFETY: `console` is a console handle returned by `_get_osfhandle`.
        if unsafe { SetConsoleMode(console, mode) } == 0 {
            Err(io_error())
        } else {
            Ok(())
        }
    }

    pub(super) fn set_raw_mode(fd: i32, enable: bool) -> Result<(), JsErrorBox> {
        let Some(console) = console_handle(fd) else {
            return Err(io_error());
        };
        let mut saved = 0u32;
        // SAFETY: valid console handle and out-pointer.
        if unsafe { GetConsoleMode(console, &raw mut saved) } == 0 {
            return Err(io_error());
        }
        if !enable {
            let previous = SAVED_MODES.lock().unwrap().remove(&fd);
            return previous.map_or(Ok(()), |mode| set_console_mode(console, mode));
        }
        // libuv's Windows raw mode is ENABLE_WINDOW_INPUT; VT input is used when
        // the console supports it. This mirrors deno_core's uv_compat tty.
        const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
        if unsafe { SetConsoleMode(console, ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT) }
            == 0
            && unsafe { SetConsoleMode(console, ENABLE_WINDOW_INPUT) } == 0
        {
            return Err(io_error());
        }
        SAVED_MODES.lock().unwrap().entry(fd).or_insert(saved);
        Ok(())
    }

    pub(super) fn is_raw(fd: i32) -> bool {
        SAVED_MODES.lock().unwrap().contains_key(&fd)
    }

    pub(super) fn restore_terminals() {
        let saved = std::mem::take(&mut *SAVED_MODES.lock().unwrap());
        for (fd, mode) in saved {
            if let Some(console) = console_handle(fd) {
                let _ = set_console_mode(console, mode);
            }
        }
    }
}
