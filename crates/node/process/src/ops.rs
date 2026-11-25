use deno_core::uv_compat;
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
    // Node restores the terminal from its exit path; `uv_compat` owns the saved mode.
    uv_compat::uv_tty_reset_mode();
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

#[cfg(target_os = "linux")]
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

#[cfg(all(unix, not(target_os = "linux")))]
#[cfg(target_vendor = "apple")]
#[expect(
    deprecated,
    reason = "libc points at the `mach2` crate for `mach_task_self`; the port is the \
              process-wide constant behind the C macro, so one call does not justify the dependency"
)]
fn memory() -> String {
    // No `/proc` here; `task_info` on the current task reports the real current
    // resident size.
    // SAFETY: `mach_task_basic_info_data_t` is a plain-data struct for which
    // all-zero is a valid value.
    let mut info: libc::mach_task_basic_info_data_t = unsafe { std::mem::zeroed() };
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    // SAFETY: `task_info` writes `mach_task_basic_info` into the zeroed buffer,
    // bounded by `count`; `mach_task_self` is always a valid port.
    let current_rss = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            (&raw mut info).cast(),
            &raw mut count,
        )
    } == libc::KERN_SUCCESS;
    if current_rss {
        // The struct is only 4-byte aligned, so the 8-byte field is copied out
        // instead of referenced; `format!` would borrow it (E0793).
        let resident_size = info.resident_size;
        format!("{resident_size}\0\0\0\0\0")
    } else {
        // `task_info` on the current task cannot realistically fail; if it ever
        // does, fall back to `ru_maxrss` (bytes on macOS) — a high-water mark
        // rather than a fake zero.
        format!("{}\0\0\0\0\0", rusage().map_or(0, |usage| usage.ru_maxrss))
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
#[cfg(not(target_vendor = "apple"))]
fn memory() -> String {
    // No `/proc` and no lightweight current-RSS call here; `ru_maxrss` (KiB) is a
    // high-water approximation of the resident size, not a fake zero.
    let rss = rusage().map_or(0, |usage| usage.ru_maxrss * 1024);
    format!("{rss}\0\0\0\0\0")
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

/// The process's terminals, initialized on first use.
///
/// `uv_compat` owns the saved terminal state, so raw mode, the window size and
/// `uv_tty_reset_mode` on the exit path all agree. The handles live for the
/// process lifetime, like Node's stdio `TTYWrap`s; libuv's reset is the
/// teardown, not a close. Their loop is private to the registry so it cannot be
/// freed before the handles that point at it when a runtime is dropped.
mod tty {
    use std::collections::HashMap;
    use std::ffi::c_int;
    use std::sync::{LazyLock, Mutex};

    use deno_core::uv_compat::{
        self, uv_guess_handle, uv_handle_type, uv_loop_t, uv_tty_init, uv_tty_mode_t, uv_tty_t,
    };

    struct Terminals {
        // Boxed so the address the handles store stays put.
        loop_: Box<uv_loop_t>,
        handles: HashMap<c_int, Box<uv_tty_t>>,
    }

    #[expect(
        clippy::non_send_fields_in_send_ty,
        reason = "the mutex serializes access; the raw pointers are never shared"
    )]
    // SAFETY: `TERMINALS` serializes every use of the loop and its handles, so
    // they are never used from two threads at once. They are only `!Send`
    // because libuv's types hold raw pointers and `Cell`s.
    unsafe impl Send for Terminals {}

    static TERMINALS: LazyLock<Mutex<Terminals>> = LazyLock::new(|| {
        // SAFETY: zeroed memory is the uninitialized state `uv_loop_init` expects.
        let mut loop_ = Box::new(unsafe { std::mem::zeroed::<uv_loop_t>() });
        // SAFETY: `loop_` is valid, writable memory and outlives the handles.
        assert_eq!(unsafe { uv_compat::uv_loop_init(&raw mut *loop_) }, 0);
        Mutex::new(Terminals {
            loop_,
            handles: HashMap::new(),
        })
    });

    fn with_handle<R>(fd: c_int, use_handle: impl FnOnce(&mut uv_tty_t) -> R) -> Option<R> {
        if uv_guess_handle(fd) != uv_handle_type::UV_TTY {
            return None;
        }
        let mut terminals = TERMINALS.lock().unwrap();
        let terminals = &mut *terminals;
        if !terminals.handles.contains_key(&fd) {
            let mut handle = Box::new(uv_compat::new_tty());
            let loop_ = std::ptr::from_mut(&mut *terminals.loop_);
            // SAFETY: both pointers are valid and `fd` is a terminal, which is what
            // `uv_tty_init` requires.
            if unsafe { uv_tty_init(loop_, &raw mut *handle, fd, 0) } != 0 {
                return None;
            }
            terminals.handles.insert(fd, handle);
        }
        Some(use_handle(terminals.handles.get_mut(&fd)?))
    }

    pub(super) fn size(fd: c_int) -> (c_int, c_int) {
        let mut columns = 0;
        let mut rows = 0;
        with_handle(fd, |tty| {
            // SAFETY: the registry lock guards the initialized handle, and both outputs are valid.
            unsafe { uv_compat::uv_tty_get_winsize(tty, &raw mut columns, &raw mut rows) }
        });
        (columns, rows)
    }

    pub(super) fn set_mode(fd: c_int, mode: uv_tty_mode_t) -> c_int {
        with_handle(fd, |tty| {
            // SAFETY: the registry lock guards the initialized handle.
            unsafe { uv_compat::uv_tty_set_mode(tty, mode) }
        })
        .unwrap_or(uv_compat::UV_EINVAL)
    }
}

/// Map Node's `setRawMode` argument to the libuv mode it selects.
fn tty_mode(mode: &str) -> uv_compat::uv_tty_mode_t {
    match mode {
        "io" => uv_compat::uv_tty_mode_t::UV_TTY_MODE_IO,
        // Node's raw mode is libuv's `RAW_VT`: on Windows it asks the console for
        // VT input, so key presses arrive as escape sequences; on Unix the two
        // raw modes are the same.
        "raw" => uv_compat::uv_tty_mode_t::UV_TTY_MODE_RAW_VT,
        _ => uv_compat::uv_tty_mode_t::UV_TTY_MODE_NORMAL,
    }
}

#[op2]
#[string]
pub(crate) fn op_process_tty_size(#[smi] fd: i32) -> String {
    let (columns, rows) = tty::size(fd);
    format!("{columns}\0{rows}")
}

#[op2(nofast)]
pub(crate) fn op_process_set_raw_mode(
    #[smi] fd: i32,
    #[string] mode: &str,
) -> Result<(), JsErrorBox> {
    match tty::set_mode(fd, tty_mode(mode)) {
        0 => Ok(()),
        status => Err(JsErrorBox::new("Error", status.to_string())),
    }
}

/// Deno's Windows process layer: process times and memory counters through the
/// same `windows-sys` APIs `ext/node/ops/process.rs` uses.
#[cfg(windows)]
mod win32 {
    use std::mem::{MaybeUninit, size_of};

    use deno_error::JsErrorBox;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE, TerminateProcess,
    };

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
}
