//! Ops backing `node:cluster`.
//!
//! The primary and the worker both use the same two tables: one entry per spawned worker
//! process (child handle, stdio pipes) and one entry per IPC channel. They share an id so a
//! worker process has a single number on both sides of the channel.

use std::collections::HashMap;
use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};

pub(crate) const OPS: &[OpDecl] = &[
    op_cluster_spawn(),
    op_cluster_clear_env(),
    op_cluster_allocate_port(),
    op_cluster_channel_adopt(),
    op_cluster_channel_read(),
    op_cluster_channel_write(),
    op_cluster_channel_write_sync(),
    op_cluster_channel_close(),
    op_cluster_wait(),
    op_cluster_kill(),
    op_cluster_read_pipe(),
    op_cluster_write_stdin(),
    op_cluster_close_stdin(),
];

/// The descriptor the child side of the channel is moved to before `exec`.
const CHANNEL_FD: RawFd = 3;
const PIPE_CHUNK: usize = 65536;

struct WorkerEntry {
    child: Arc<tokio::sync::Mutex<Child>>,
    pid: u32,
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    stdout: Arc<tokio::sync::Mutex<Option<ChildStdout>>>,
    stderr: Arc<tokio::sync::Mutex<Option<ChildStderr>>>,
}

struct ChannelEntry {
    stream: UnixStream,
    write: tokio::sync::Mutex<()>,
}

fn workers() -> &'static Mutex<HashMap<u32, Arc<WorkerEntry>>> {
    static WORKERS: OnceLock<Mutex<HashMap<u32, Arc<WorkerEntry>>>> = OnceLock::new();
    WORKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn channels() -> &'static Mutex<HashMap<u32, Arc<ChannelEntry>>> {
    static CHANNELS: OnceLock<Mutex<HashMap<u32, Arc<ChannelEntry>>>> = OnceLock::new();
    CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

static ENTRIES: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

fn cluster_error(code: &str, detail: impl std::fmt::Display) -> JsErrorBox {
    JsErrorBox::new("Error", format!("{code}\0{detail}"))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "`io::Error` is a `map_err` sink; taking a reference would force a closure at every call site"
)]
fn io_error(error: io::Error) -> JsErrorBox {
    let errno = error.raw_os_error().unwrap_or(libc::EIO);
    JsErrorBox::new("Error", format!("-{errno}\0{error}"))
}

fn worker_of(id: u32) -> Result<Arc<WorkerEntry>, JsErrorBox> {
    workers()
        .lock()
        .expect("worker table")
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| cluster_error("ESRCH", "cluster worker not found"))
}

fn channel_of(id: u32) -> Result<Arc<ChannelEntry>, JsErrorBox> {
    channels()
        .lock()
        .expect("channel table")
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| cluster_error("EPIPE", "cluster channel is closed"))
}

#[op2]
#[string]
fn op_cluster_spawn(
    #[string] argv: &str,
    #[string] env: &str,
    #[string] cwd: &str,
    silent: bool,
    worker_id: u32,
) -> Result<String, JsErrorBox> {
    let mut parts = argv.split('\0');
    let program = parts.next().unwrap_or_default();
    if program.is_empty() {
        return Err(cluster_error("ERR_INVALID_ARG_VALUE", "empty command"));
    }
    let rest: Vec<String> = parts.map(str::to_owned).collect();

    let (parent, child_end) = std::os::unix::net::UnixStream::pair().map_err(io_error)?;
    parent.set_nonblocking(true).map_err(io_error)?;
    let channel_fd = child_end.as_raw_fd();

    let mut command = tokio::process::Command::new(program);
    command
        .args(&rest)
        .env("NODE_UNIQUE_ID", worker_id.to_string())
        .env("NODE_CHANNEL_FD", CHANNEL_FD.to_string());
    for pair in env.split('\0').filter(|pair| !pair.is_empty()) {
        if let Some((key, value)) = pair.split_once('=') {
            command.env(key, value);
        }
    }
    if !cwd.is_empty() {
        command.current_dir(cwd);
    }
    command.stdin(stdio(silent));
    command.stdout(stdio(silent));
    command.stderr(stdio(silent));

    // SAFETY: the hook runs in the child between fork and exec. `dup2` and `fcntl` are
    // async-signal-safe and only touch descriptors, which is what `pre_exec` requires.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            if channel_fd != CHANNEL_FD {
                if libc::dup2(channel_fd, CHANNEL_FD) == -1 {
                    return Err(io::Error::last_os_error());
                }
            } else if libc::fcntl(CHANNEL_FD, libc::F_SETFD, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = command.spawn().map_err(io_error)?;
    drop(child_end);
    let pid = child.id().unwrap_or_default();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stream = UnixStream::from_std(parent).map_err(io_error)?;

    let id = ENTRIES.next();
    workers().lock().expect("worker table").insert(
        id,
        Arc::new(WorkerEntry {
            child: Arc::new(tokio::sync::Mutex::new(child)),
            pid,
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            stdout: Arc::new(tokio::sync::Mutex::new(stdout)),
            stderr: Arc::new(tokio::sync::Mutex::new(stderr)),
        }),
    );
    channels().lock().expect("channel table").insert(
        id,
        Arc::new(ChannelEntry {
            stream,
            write: tokio::sync::Mutex::new(()),
        }),
    );
    Ok(format!("{id}\0{pid}"))
}

#[op2(fast)]
fn op_cluster_clear_env() {
    // SAFETY: a cluster worker calls this once during startup, before user code can spawn
    // children. These two variables belong to the worker bootstrap and are not read by any
    // other thread in the runtime after this point.
    unsafe {
        std::env::remove_var("NODE_UNIQUE_ID");
        std::env::remove_var("NODE_CHANNEL_FD");
    }
}

fn stdio(silent: bool) -> Stdio {
    if silent {
        Stdio::piped()
    } else {
        Stdio::inherit()
    }
}

#[op2(fast)]
fn op_cluster_allocate_port(#[string] host: &str) -> Result<u16, JsErrorBox> {
    let listener = std::net::TcpListener::bind((host, 0)).map_err(io_error)?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(io_error)
}

#[op2(fast)]
fn op_cluster_channel_adopt(fd: i32) -> Result<u32, JsErrorBox> {
    // SAFETY: `fcntl(F_GETFD)` only inspects the descriptor.
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 {
        return Err(io_error(io::Error::last_os_error()));
    }
    let mut socket_type: libc::c_int = 0;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`size_of::<libc::c_int>()` is 4 on every supported platform"
    )]
    let mut length = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `getsockopt` writes at most `length` bytes into `socket_type`, and both
    // pointers stay valid for the call.
    let got = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut socket_type).cast(),
            &raw mut length,
        )
    };
    if got == -1 {
        return Err(io_error(io::Error::last_os_error()));
    }
    if socket_type != libc::SOCK_STREAM {
        return Err(cluster_error(
            "ENOTSOCK",
            "the channel descriptor is not a stream socket",
        ));
    }

    // SAFETY: the descriptor is open and is a stream socket, and ownership moves into the
    // `UnixStream` from here on.
    let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
    stream.set_nonblocking(true).map_err(io_error)?;
    let stream = UnixStream::from_std(stream).map_err(io_error)?;

    let id = ENTRIES.next();
    channels().lock().expect("channel table").insert(
        id,
        Arc::new(ChannelEntry {
            stream,
            write: tokio::sync::Mutex::new(()),
        }),
    );
    Ok(id)
}

#[op2(async(lazy), fast)]
#[string]
async fn op_cluster_channel_read(id: u32) -> Result<String, JsErrorBox> {
    let entry = channel_of(id)?;
    let mut header = [0u8; 4];
    if !fill_exact(&entry.stream, &mut header)
        .await
        .map_err(io_error)?
    {
        return Ok(String::new());
    }
    let length =
        usize::try_from(u32::from_be_bytes(header)).expect("a u32 frame length fits in usize");
    let mut payload = vec![0u8; length];
    if !fill_exact(&entry.stream, &mut payload)
        .await
        .map_err(io_error)?
    {
        return Ok(String::new());
    }
    String::from_utf8(payload)
        .map_err(|_| cluster_error("EINVAL", "the channel frame is not UTF-8"))
}

#[op2(async(lazy), fast)]
async fn op_cluster_channel_write(id: u32, #[string] frame: String) -> Result<(), JsErrorBox> {
    let entry = channel_of(id)?;
    let _guard = entry.write.lock().await;
    let length = u32::try_from(frame.len())
        .map_err(|_| cluster_error("E2BIG", "the channel frame is larger than 4 GiB"))?;
    let mut bytes = Vec::with_capacity(4 + frame.len());
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(frame.as_bytes());
    write_all(&entry.stream, &bytes).await.map_err(io_error)
}

#[op2(fast)]
fn op_cluster_channel_write_sync(id: u32, #[string] frame: &str) -> Result<(), JsErrorBox> {
    // Used once per worker for the `online` notification, before user code runs. A worker
    // that immediately enters an infinite loop never polls the async write op, but its
    // online message still has to reach the primary, which is the behaviour Node's
    // libuv-backed channel has. The write is tiny and the primary is reading, so blocking
    // for the socket to become writable is bounded in practice.
    let entry = channel_of(id)?;
    let length = u32::try_from(frame.len())
        .map_err(|_| cluster_error("E2BIG", "the channel frame is larger than 4 GiB"))?;
    let mut bytes = Vec::with_capacity(4 + frame.len());
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(frame.as_bytes());

    let fd = entry.stream.as_raw_fd();
    let mut written = 0;
    while written < bytes.len() {
        // SAFETY: `write(2)` reads the buffer and reports a closed socket through its return
        // value; the descriptor is owned by `entry.stream`.
        let count =
            unsafe { libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        if count > 0 {
            written += count.unsigned_abs();
            continue;
        }
        if count == 0 {
            return Err(cluster_error("EPIPE", "channel closed"));
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            let mut descriptor = libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: `poll` reads and writes only the single `pollfd` supplied here.
            if unsafe { libc::poll(&raw mut descriptor, 1, -1) } == -1 {
                return Err(io_error(io::Error::last_os_error()));
            }
            continue;
        }
        return Err(io_error(error));
    }
    Ok(())
}

#[op2(fast)]
fn op_cluster_channel_close(id: u32) {
    let Some(entry) = channels().lock().expect("channel table").remove(&id) else {
        return;
    };
    // SAFETY: `shutdown` only reads the descriptor owned by `entry.stream`; it wakes a
    // pending read so the JS pump observes the close immediately.
    unsafe { libc::shutdown(entry.stream.as_raw_fd(), libc::SHUT_RDWR) };
}

#[op2(async(lazy), fast)]
#[string]
async fn op_cluster_wait(id: u32) -> Result<String, JsErrorBox> {
    let child = Arc::clone(&worker_of(id)?.child);
    let mut child = child.lock().await;
    let status = child.wait().await.map_err(io_error)?;
    Ok(format!(
        "{}\0{}",
        status.code().unwrap_or(-1),
        status.signal().unwrap_or(0)
    ))
}

#[op2(fast)]
fn op_cluster_kill(id: u32, signal: i32) -> Result<(), JsErrorBox> {
    let pid = worker_of(id)?.pid.cast_signed();
    // SAFETY: `kill(2)` takes plain integers and reports a bad pid or signal through its
    // return value instead of being undefined behaviour.
    if unsafe { libc::kill(pid, signal) } == -1 {
        return Err(io_error(io::Error::last_os_error()));
    }
    Ok(())
}

#[op2(async(lazy), fast)]
#[buffer]
async fn op_cluster_read_pipe(id: u32, which: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let entry = worker_of(id)?;
    let mut buffer = vec![0u8; PIPE_CHUNK];
    let count = if which == 1 {
        read_pipe(&entry.stdout, &mut buffer).await?
    } else {
        read_pipe(&entry.stderr, &mut buffer).await?
    };
    buffer.truncate(count);
    Ok(buffer.into_boxed_slice())
}

async fn read_pipe<T: AsyncRead + Unpin>(
    pipe: &tokio::sync::Mutex<Option<T>>,
    buffer: &mut [u8],
) -> Result<usize, JsErrorBox> {
    match pipe.lock().await.as_mut() {
        Some(pipe) => pipe.read(buffer).await.map_err(io_error),
        None => Ok(0),
    }
}

#[op2(async(lazy), fast)]
async fn op_cluster_write_stdin(id: u32, #[buffer(copy)] data: Vec<u8>) -> Result<(), JsErrorBox> {
    let entry = worker_of(id)?;
    let mut pipe = entry.stdin.lock().await;
    match pipe.as_mut() {
        Some(stdin) => stdin.write_all(&data).await.map_err(io_error),
        None => Err(cluster_error("EPIPE", "the worker stdin is closed")),
    }
}

#[op2(async(lazy), fast)]
async fn op_cluster_close_stdin(id: u32) -> Result<(), JsErrorBox> {
    let entry = worker_of(id)?;
    entry.stdin.lock().await.take();
    Ok(())
}

async fn fill_exact(stream: &UnixStream, buffer: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        stream.readable().await?;
        match stream.try_read(&mut buffer[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

async fn write_all(stream: &UnixStream, bytes: &[u8]) -> io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        stream.writable().await?;
        match stream.try_write(&bytes[written..]) {
            Ok(0) => return Err(io::ErrorKind::BrokenPipe.into()),
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
