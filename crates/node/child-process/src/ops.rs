use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;
use std::io::Write as _;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};

pub(crate) const OPS: &[OpDecl] = &[
    op_child_spawn(),
    op_child_read(),
    op_child_write(),
    op_child_close_stdin(),
    op_child_wait(),
    op_child_kill(),
    op_child_pid(),
    op_child_spawn_sync(),
];

struct Entry {
    child: Arc<tokio::sync::Mutex<Child>>,
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    stdout: Arc<tokio::sync::Mutex<Option<ChildStdout>>>,
    stderr: Arc<tokio::sync::Mutex<Option<ChildStderr>>>,
    /// Set once `wait` reaps the process. The table entry is dropped once this is set and
    /// both output pipes are drained (see `reap_if_drained`), so per-request spawners do
    /// not accumulate a `Child` and its pipe handles forever.
    exited: AtomicBool,
}

fn table() -> &'static Mutex<HashMap<u32, Arc<Entry>>> {
    static TABLE: OnceLock<Mutex<HashMap<u32, Arc<Entry>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

static CHILDREN: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

fn child_error(code: &str, detail: impl std::fmt::Display) -> JsErrorBox {
    JsErrorBox::new("Error", format!("{code}\0{detail}"))
}

struct Spawn {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: Option<String>,
}

impl Spawn {
    fn parse(argv: &str, env: &str, cwd: &str) -> Result<Self, JsErrorBox> {
        let mut parts = argv.split('\0');
        let program = parts.next().unwrap_or_default();
        if program.is_empty() {
            return Err(child_error("ERR_INVALID_ARG_VALUE", "empty command"));
        }
        Ok(Self {
            program: program.to_owned(),
            args: parts.map(str::to_owned).collect(),
            env: env
                .split('\0')
                .filter_map(|entry| entry.split_once('='))
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
            cwd: (!cwd.is_empty()).then(|| cwd.to_owned()),
        })
    }

    fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !self.env.is_empty() {
            command
                .env_clear()
                .envs(self.env.iter().map(|(key, value)| (key, value)));
        }
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
    }
}

fn spawn_error(error: std::io::Error) -> JsErrorBox {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        _ => "EIO",
    };
    child_error(code, error)
}

#[op2(fast)]
fn op_child_spawn(
    #[string] argv: &str,
    #[string] env: &str,
    #[string] cwd: &str,
) -> Result<u32, JsErrorBox> {
    let request = Spawn::parse(argv, env, cwd)?;
    let mut command = tokio::process::Command::from(request.command());
    command.kill_on_drop(false);

    let mut child = command.spawn().map_err(spawn_error)?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let id = CHILDREN.next();
    table().lock().expect("child table").insert(
        id,
        Arc::new(Entry {
            child: Arc::new(tokio::sync::Mutex::new(child)),
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            stdout: Arc::new(tokio::sync::Mutex::new(stdout)),
            stderr: Arc::new(tokio::sync::Mutex::new(stderr)),
            exited: AtomicBool::new(false),
        }),
    );
    Ok(id)
}

#[op2(async(lazy), fast)]
#[buffer]
async fn op_child_read(id: u32, which: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let entry = entry_of(id)?;
    let mut buffer = vec![0u8; 65536];
    let count = if which == 1 {
        read_pipe(&entry.stdout, &mut buffer).await?
    } else {
        read_pipe(&entry.stderr, &mut buffer).await?
    };
    reap_if_drained(id, &entry);
    buffer.truncate(count);
    Ok(buffer.into_boxed_slice())
}

async fn read_pipe<T: AsyncRead + Unpin>(
    pipe: &tokio::sync::Mutex<Option<T>>,
    buffer: &mut [u8],
) -> Result<usize, JsErrorBox> {
    let mut guard = pipe.lock().await;
    let result = match guard.as_mut() {
        Some(pipe) => pipe.read(buffer).await,
        None => Ok(0),
    };
    // EOF and a failed read both mean the pipe can never produce more data, so dropping
    // it here advances the entry toward being reaped (see `reap_if_drained`).
    if matches!(result, Ok(0) | Err(_)) {
        guard.take();
    }
    result.map_err(|error| child_error("EIO", error))
}

#[op2(async(lazy), fast)]
async fn op_child_write(id: u32, #[buffer(copy)] data: Vec<u8>) -> Result<u32, JsErrorBox> {
    let entry = entry_of(id)?;
    let mut pipe = entry.stdin.lock().await;
    match pipe.as_mut() {
        Some(stdin) => {
            stdin
                .write_all(&data)
                .await
                .map_err(|error| child_error("EPIPE", error))?;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "op buffers come from V8 typed arrays, whose length stays below `u32::MAX`"
            )]
            let written = data.len() as u32;
            Ok(written)
        }
        None => Err(child_error("EPIPE", "stdin is closed")),
    }
}

#[op2(async(lazy), fast)]
async fn op_child_close_stdin(id: u32) -> Result<(), JsErrorBox> {
    let entry = entry_of(id)?;
    let mut pipe = entry.stdin.lock().await;
    pipe.take();
    Ok(())
}

/// Removes the table entry once `wait` has reaped the child and both output pipes are
/// drained. The JS wrapper always pumps both pipes to EOF, so the last event — the final
/// pipe read or the wait — reaps the entry and releases the `Child` and its descriptors.
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

#[op2(async(lazy), fast)]
#[string]
async fn op_child_wait(id: u32) -> Result<String, JsErrorBox> {
    let entry = entry_of(id)?;
    let status = {
        let mut child = entry.child.lock().await;
        child
            .wait()
            .await
            .map_err(|error| child_error("ECHILD", error))?
    };
    entry.exited.store(true, Ordering::Release);
    reap_if_drained(id, &entry);
    Ok(format!(
        "{}\0{}",
        status.code().unwrap_or(-1),
        exit_signal(status)
    ))
}

/// The signal number that terminated the child, or 0 for a plain exit.
#[cfg(unix)]
fn exit_signal(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt as _;
    status.signal().unwrap_or(0)
}

#[cfg(windows)]
fn exit_signal(_status: std::process::ExitStatus) -> i32 {
    // Windows processes always terminate through their exit code; `TerminateProcess`
    // surfaces as a code, never as a signal number.
    0
}

fn child_pid(id: u32) -> Result<u32, JsErrorBox> {
    let entry = entry_of(id)?;
    let child = entry
        .child
        .try_lock()
        .map_err(|_| child_error("EBUSY", "the process is being waited"))?;
    child
        .id()
        .ok_or_else(|| child_error("ESRCH", "the process has exited"))
}

#[op2(fast)]
fn op_child_pid(id: u32) -> Result<u32, JsErrorBox> {
    child_pid(id)
}

#[cfg(unix)]
#[op2(fast)]
fn op_child_kill(id: u32, signal: i32) -> Result<(), JsErrorBox> {
    let pid = child_pid(id)?;
    // SAFETY: `kill(2)` takes plain integers; a wrong pid or signal is reported through
    // the return value instead of being undefined behaviour.
    if unsafe { libc::kill(pid as libc::pid_t, signal) } == -1 {
        return Err(spawn_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(windows)]
#[op2(fast)]
fn op_child_kill(id: u32, signal: i32) -> Result<(), JsErrorBox> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    };

    let pid = child_pid(id)?;
    let access = if signal == 0 {
        PROCESS_QUERY_LIMITED_INFORMATION
    } else {
        PROCESS_TERMINATE
    };
    // SAFETY: plain Win32 call; the returned handle is closed before returning.
    let process = unsafe { OpenProcess(access, 0, pid) };
    if process.is_null() {
        return Err(spawn_error(std::io::Error::last_os_error()));
    }
    let result = if signal == 0 {
        Ok(())
    // SAFETY: `process` is the handle opened above.
    } else if unsafe { TerminateProcess(process, 1) } == 0 {
        Err(spawn_error(std::io::Error::last_os_error()))
    } else {
        Ok(())
    };
    // SAFETY: `process` is a valid handle opened by `OpenProcess`.
    unsafe { CloseHandle(process) };
    result
}

#[op2]
#[buffer]
fn op_child_spawn_sync(
    #[string] argv: &str,
    #[string] env: &str,
    #[string] cwd: &str,
    #[buffer(copy)] input: Vec<u8>,
) -> Result<Box<[u8]>, JsErrorBox> {
    let request = Spawn::parse(argv, env, cwd)?;
    let mut child = request.command().spawn().map_err(spawn_error)?;
    let pid = child.id();
    let stdin = child.stdin.take();
    // Writing stdin in-line would deadlock with a child that fills an output pipe while
    // the parent is still writing, so the input is written from a side thread while the
    // parent drains the pipes. Dropping the handle inside the thread closes the pipe and
    // gives the child EOF even when the write fails.
    let writer =
        std::thread::spawn(move || stdin.map_or(Ok(()), |mut stdin| stdin.write_all(&input)));
    let output = child
        .wait_with_output()
        .map_err(|error| child_error("ECHILD", error))?;
    let written = writer.join().expect("the stdin writer thread cannot panic");
    if let Err(error) = written {
        return Err(child_error("EPIPE", error));
    }
    // The result header is `status` (i32), `signal` (i32), `pid` (u32) and the two output
    // lengths (u32 each), little-endian; the JS reader decodes the same layout.
    let code = output.status.code().unwrap_or(-1);
    let signal = exit_signal(output.status);
    let mut payload = Vec::with_capacity(20 + output.stdout.len() + output.stderr.len());
    payload.extend_from_slice(&code.to_le_bytes());
    payload.extend_from_slice(&signal.to_le_bytes());
    payload.extend_from_slice(&pid.to_le_bytes());
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the spawnSync result header encodes both lengths as u32, so a longer output cannot be represented on the wire"
    )]
    let stdout_length = output.stdout.len() as u32;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the spawnSync result header encodes both lengths as u32, so a longer output cannot be represented on the wire"
    )]
    let stderr_length = output.stderr.len() as u32;
    payload.extend_from_slice(&stdout_length.to_le_bytes());
    payload.extend_from_slice(&stderr_length.to_le_bytes());
    payload.extend_from_slice(&output.stdout);
    payload.extend_from_slice(&output.stderr);
    Ok(payload.into_boxed_slice())
}

fn entry_of(id: u32) -> Result<Arc<Entry>, JsErrorBox> {
    table()
        .lock()
        .expect("child table")
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| child_error("ESRCH", "child process not found"))
}
