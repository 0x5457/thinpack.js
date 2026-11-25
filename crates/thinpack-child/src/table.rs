use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::Mutex as AsyncMutex;

use crate::ChildError;
use crate::spawn::{Exited, Spawned};

const PIPE_CHUNK: usize = 65536;

/// Which output pipe an op reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

struct Entry {
    child: AsyncMutex<Child>,
    pid: u32,
    stdin: AsyncMutex<Option<ChildStdin>>,
    stdout: AsyncMutex<Option<ChildStdout>>,
    stderr: AsyncMutex<Option<ChildStderr>>,
    /// Set once `wait` reaped the process. The entry is dropped once this is set and both
    /// output pipes are drained (see `reap_if_drained`), so a program that spawns per request
    /// does not accumulate a `Child` and its pipe handles for every finished process.
    exited: AtomicBool,
}

fn table() -> &'static Mutex<HashMap<u32, Arc<Entry>>> {
    static TABLE: OnceLock<Mutex<HashMap<u32, Arc<Entry>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

static ENTRIES: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

pub(crate) fn register(child: Child) -> Result<Spawned, ChildError> {
    let pid = child.id().ok_or_else(|| {
        ChildError::new("ESRCH", "the process exited before it could be registered")
    })?;
    let mut child = child;
    let entry = Entry {
        stdin: AsyncMutex::new(child.stdin.take()),
        stdout: AsyncMutex::new(child.stdout.take()),
        stderr: AsyncMutex::new(child.stderr.take()),
        child: AsyncMutex::new(child),
        pid,
        exited: AtomicBool::new(false),
    };
    let id = ENTRIES.next();
    table()
        .lock()
        .expect("child table")
        .insert(id, Arc::new(entry));
    Ok(Spawned { id, pid })
}

fn entry_of(id: u32) -> Result<Arc<Entry>, ChildError> {
    table()
        .lock()
        .expect("child table")
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| ChildError::new("ESRCH", "child process not found"))
}

/// Reads one chunk from the child's stdout or stderr; an empty chunk is end of file.
///
/// # Errors
///
/// Returns `ESRCH` once the entry is reaped and `EIO` when the pipe fails.
pub(crate) async fn read(id: u32, stream: Stream) -> Result<Box<[u8]>, ChildError> {
    let entry = entry_of(id)?;
    let mut buffer = vec![0u8; PIPE_CHUNK];
    let count = match stream {
        Stream::Stdout => read_pipe(&entry.stdout, &mut buffer).await?,
        Stream::Stderr => read_pipe(&entry.stderr, &mut buffer).await?,
    };
    reap_if_drained(id, &entry);
    buffer.truncate(count);
    Ok(buffer.into_boxed_slice())
}

async fn read_pipe<T: AsyncRead + Unpin>(
    pipe: &AsyncMutex<Option<T>>,
    buffer: &mut [u8],
) -> Result<usize, ChildError> {
    let mut guard = pipe.lock().await;
    let result = match guard.as_mut() {
        Some(pipe) => pipe.read(buffer).await,
        None => Ok(0),
    };
    // EOF and a failed read both mean the pipe can never produce more data, so dropping it
    // here advances the entry toward being reaped (see `reap_if_drained`).
    if matches!(result, Ok(0) | Err(_)) {
        guard.take();
    }
    result.map_err(|error| ChildError::io("EIO", error))
}

/// Writes data to the child's stdin.
///
/// # Errors
///
/// Returns `EPIPE` once stdin is closed or the pipe fails.
pub(crate) async fn write(id: u32, data: &[u8]) -> Result<u32, ChildError> {
    let entry = entry_of(id)?;
    let mut pipe = entry.stdin.lock().await;
    let Some(stdin) = pipe.as_mut() else {
        return Err(ChildError::new("EPIPE", "stdin is closed"));
    };
    stdin
        .write_all(data)
        .await
        .map_err(|error| ChildError::io("EPIPE", error))?;
    u32::try_from(data.len())
        .map_err(|_| ChildError::new("E2BIG", "the write is larger than 4 GiB"))
}

/// Closes the child's stdin, which gives it end of file.
///
/// # Errors
///
/// Returns `ESRCH` once the entry is reaped.
pub(crate) async fn close_stdin(id: u32) -> Result<(), ChildError> {
    entry_of(id)?.stdin.lock().await.take();
    Ok(())
}

/// Waits for the child and reaps its table entry once the output pipes are drained.
///
/// # Errors
///
/// Returns `ESRCH` for an unknown entry and `ECHILD` when the wait itself fails.
pub(crate) async fn wait(id: u32) -> Result<Exited, ChildError> {
    let entry = entry_of(id)?;
    let status = entry
        .child
        .lock()
        .await
        .wait()
        .await
        .map_err(|error| ChildError::io("ECHILD", error))?;
    entry.exited.store(true, Ordering::Release);
    reap_if_drained(id, &entry);
    Ok(Exited::new(status))
}

/// The pid the child was spawned with, which stays valid after it exits.
///
/// # Errors
///
/// Returns `ESRCH` once the entry is reaped.
pub fn pid(id: u32) -> Result<u32, ChildError> {
    Ok(entry_of(id)?.pid)
}

/// Signals the child.
///
/// # Errors
///
/// Returns `ESRCH` once the process is gone, `EPERM` when the signal is not permitted, and
/// `EIO` for any other OS failure.
pub fn kill(id: u32, signal: i32) -> Result<(), ChildError> {
    let entry = entry_of(id)?;
    // A process that was reaped, or one whose `wait` has already returned, is gone: report
    // the same `ESRCH` a vanished entry does, which both surfaces turn into `kill()`
    // returning false.
    if entry.exited.load(Ordering::Acquire) {
        return Err(ChildError::new("ESRCH", "the process has exited"));
    }
    terminate(entry.pid, signal)
}

/// Removes the table entry once `wait` has reaped the child and both output pipes are
/// drained. The JavaScript wrapper always pumps both pipes to EOF, so the last event — the
/// final pipe read or the wait — reaps the entry and releases the `Child` and its
/// descriptors.
fn reap_if_drained(id: u32, entry: &Entry) {
    if !entry.exited.load(Ordering::Acquire) {
        return;
    }
    let Ok(stdout) = entry.stdout.try_lock() else {
        return;
    };
    if stdout.is_some() {
        return;
    }
    let Ok(stderr) = entry.stderr.try_lock() else {
        return;
    };
    if stderr.is_some() {
        return;
    }
    table().lock().expect("child table").remove(&id);
}

#[cfg(unix)]
fn terminate(pid: u32, signal: i32) -> Result<(), ChildError> {
    // A kernel never hands out a pid above the `pid_t` maximum, so the value the child
    // reported does round-trip.
    let pid = pid as libc::pid_t;
    // SAFETY: `kill(2)` takes plain integers; a wrong pid or signal is reported through the
    // return value instead of being undefined behaviour.
    if unsafe { libc::kill(pid, signal) } == -1 {
        return Err(ChildError::signal(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(windows)]
fn terminate(pid: u32, signal: i32) -> Result<(), ChildError> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    };

    let access = if signal == 0 {
        PROCESS_QUERY_LIMITED_INFORMATION
    } else {
        PROCESS_TERMINATE
    };
    // SAFETY: plain Win32 call; the returned handle is closed before returning.
    let process = unsafe { OpenProcess(access, 0, pid) };
    if process.is_null() {
        return Err(ChildError::signal(std::io::Error::last_os_error()));
    }
    let result = if signal == 0 {
        Ok(())
    // SAFETY: `process` is the handle opened above.
    } else if unsafe { TerminateProcess(process, 1) } == 0 {
        Err(ChildError::signal(std::io::Error::last_os_error()))
    } else {
        Ok(())
    };
    // SAFETY: `process` is a valid handle opened by `OpenProcess`.
    unsafe { CloseHandle(process) };
    result
}
