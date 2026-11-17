// `op2(async(lazy))` only accepts an `async fn`, so ops whose body has nothing to await
// still have to be async.
#![expect(
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "`op2(async(lazy))` rejects a non-async op fn"
)]

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::{Arc, Mutex, OnceLock};

use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;
use tokio::io::unix::AsyncFd;
use tokio::net::{TcpStream, UnixStream};

static CONNECT_CANCELS: thinpack_core::CancelTable = thinpack_core::CancelTable::new();
static READ_CANCELS: thinpack_core::CancelTable = thinpack_core::CancelTable::new();
static ACCEPT_CANCELS: thinpack_core::CancelTable = thinpack_core::CancelTable::new();

pub(crate) const OPS: &[OpDecl] = &[
    op_net_listen(),
    op_net_listen_unix(),
    op_net_listen_fd(),
    op_net_accept(),
    op_net_close_listener(),
    op_net_listen_info(),
    op_net_connect(),
    op_net_connect_unix(),
    op_net_from_fd(),
    op_net_cancel_read(),
    op_net_cancel_connect(),
    op_net_read(),
    op_net_write(),
    op_net_shutdown(),
    op_net_close(),
    op_net_info(),
    op_net_set_no_delay(),
    op_net_set_keep_alive(),
];

enum Transport {
    Tcp(TcpStream),
    Stream(UnixStream),
    Pipe(AsyncFd<OwnedFd>),
}
macro_rules! read_ready {
    ($stream:expr, $buffer:expr) => {{
        loop {
            $stream.readable().await?;
            match $stream.try_read($buffer) {
                Ok(read) => break Ok(read),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => break Err(error),
            }
        }
    }};
}

macro_rules! write_ready {
    ($stream:expr, $data:expr) => {{
        let mut offset = 0;
        while offset < $data.len() {
            $stream.writable().await?;
            match $stream.try_write(&$data[offset..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(written) => offset += written,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }};
}

impl Transport {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Tcp(_) => "tcp",
            Self::Stream(_) | Self::Pipe(_) => "stream",
        }
    }

    async fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => read_ready!(stream, buffer),
            Self::Stream(stream) => read_ready!(stream, buffer),
            Self::Pipe(fd) => pipe_read(fd, buffer).await,
        }
    }

    async fn write_all(&self, data: &[u8]) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => write_ready!(stream, data),
            Self::Stream(stream) => write_ready!(stream, data),
            Self::Pipe(fd) => pipe_write(fd, data).await,
        }
    }

    fn fd(&self) -> RawFd {
        match self {
            Self::Tcp(stream) => stream.as_raw_fd(),
            Self::Stream(stream) => stream.as_raw_fd(),
            Self::Pipe(fd) => fd.as_raw_fd(),
        }
    }
    fn addresses(&self) -> (String, String) {
        match self {
            Self::Tcp(stream) => (
                stream
                    .local_addr()
                    .map(|address| address.to_string())
                    .unwrap_or_default(),
                stream
                    .peer_addr()
                    .map(|address| address.to_string())
                    .unwrap_or_default(),
            ),
            Self::Stream(stream) => (
                path_name(stream.local_addr()),
                path_name(stream.peer_addr()),
            ),
            Self::Pipe(_) => (String::new(), String::new()),
        }
    }
    fn shutdown(&self, how: i32) -> io::Result<()> {
        // SAFETY: `self.fd()` stays open for the whole call because `self` owns the
        // descriptor; `shutdown(2)` reports anything else through its return value.
        if unsafe { libc::shutdown(self.fd(), how) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOTSOCK) {
            Ok(())
        } else {
            Err(error)
        }
    }
    fn set_no_delay(&self, no_delay: bool) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.set_nodelay(no_delay),
            Self::Stream(_) | Self::Pipe(_) => Ok(()),
        }
    }
    fn set_keep_alive(&self, enable: bool, delay_seconds: u32) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => {
                let fd = stream.as_raw_fd();
                setsockopt_int(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_KEEPALIVE,
                    libc::c_int::from(enable),
                )?;
                if enable {
                    // A delay above `i32::MAX` reinterprets as a negative C int, which the
                    // kernel rejects with `EINVAL` -- the same error Node surfaces today.
                    setsockopt_int(
                        fd,
                        libc::IPPROTO_TCP,
                        libc::TCP_KEEPIDLE,
                        delay_seconds.cast_signed(),
                    )?;
                }
                Ok(())
            }
            Self::Stream(_) | Self::Pipe(_) => Ok(()),
        }
    }
}

async fn pipe_read(fd: &AsyncFd<OwnedFd>, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        let mut ready = fd.readable().await?;
        if let Ok(result) = ready.try_io(|inner| {
            // SAFETY: `buffer` is a live writable slice of `buffer.len()` bytes and
            // `inner.as_raw_fd()` is the descriptor this guard reported readable.
            let read =
                unsafe { libc::read(inner.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
            if read < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(read.cast_unsigned())
            }
        }) {
            return result;
        }
    }
}

async fn pipe_write(fd: &AsyncFd<OwnedFd>, data: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < data.len() {
        let mut ready = fd.writable().await?;
        match ready.try_io(|inner| {
            // SAFETY: `data[offset..]` is a live readable slice of `data.len() - offset`
            // bytes and `inner.as_raw_fd()` is the descriptor this guard reported writable.
            let written = unsafe {
                libc::write(
                    inner.as_raw_fd(),
                    data[offset..].as_ptr().cast(),
                    data.len() - offset,
                )
            };
            if written < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(written.cast_unsigned())
            }
        }) {
            Ok(Ok(0)) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(Ok(written)) => offset += written,
            Ok(Err(error)) => return Err(error),
            Err(_) => {}
        }
    }
    Ok(())
}

fn path_name(address: io::Result<tokio::net::unix::SocketAddr>) -> String {
    address
        .ok()
        .and_then(|address| address.as_pathname().map(|path| path.display().to_string()))
        .unwrap_or_default()
}

fn setsockopt_int(
    fd: RawFd,
    level: libc::c_int,
    name: libc::c_int,
    value: libc::c_int,
) -> io::Result<()> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`size_of::<libc::c_int>()` is 4 on every supported platform"
    )]
    let length = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `fd` is an open socket owned by the caller, `value` stays live for the
    // whole call and `length` describes its size exactly.
    let result = unsafe { libc::setsockopt(fd, level, name, (&raw const value).cast(), length) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn option_error(error: io::Error) -> JsErrorBox {
    if error.kind() == io::ErrorKind::Unsupported {
        JsErrorBox::new("Error", format!("ENOTSUP\0{error}"))
    } else {
        io_error(error)
    }
}

struct Connection {
    stream: Arc<Transport>,
    local: String,
    remote: String,
}

fn table() -> &'static Mutex<HashMap<u32, Connection>> {
    static TABLE: OnceLock<Mutex<HashMap<u32, Connection>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

static CONNECTIONS: thinpack_core::IdCounter = thinpack_core::IdCounter::new();
static LISTENERS: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

fn insert_connection(stream: Transport) -> u32 {
    let (local, remote) = stream.addresses();
    let id = CONNECTIONS.next();
    table().lock().expect("connection table").insert(
        id,
        Connection {
            stream: Arc::new(stream),
            local,
            remote,
        },
    );
    id
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "`io::Error` is a `map_err` sink: taking a reference would only force a closure at every call site"
)]
fn io_error(error: io::Error) -> JsErrorBox {
    let errno = error.raw_os_error().map_or(5, |code| -code);
    JsErrorBox::new("Error", errno.to_string())
}

fn cancelled(detail: &str) -> JsErrorBox {
    JsErrorBox::new("Error", format!("ECANCELED\0{detail}"))
}

fn bad_connection() -> JsErrorBox {
    JsErrorBox::new("Error", format!("-{}", libc::EBADF))
}

#[op2(fast)]
fn op_net_cancel_connect(attempt: u32) {
    let _ = CONNECT_CANCELS.cancel(attempt);
}

async fn with_connect_cancel<F>(attempt: u32, connect: F) -> Result<u32, JsErrorBox>
where
    F: Future<Output = Result<u32, JsErrorBox>>,
{
    let cancelled_signal = CONNECT_CANCELS.register(attempt);
    let result = tokio::select! {
        result = connect => result,
        _ = cancelled_signal => Err(cancelled("connect aborted")),
    };
    CONNECT_CANCELS.remove(attempt);
    result
}

#[op2(async(lazy), fast)]
async fn op_net_connect(
    #[string] host: String,
    port: u16,
    attempt: u32,
) -> Result<u32, JsErrorBox> {
    with_connect_cancel(attempt, connect_tcp(host, port)).await
}

async fn connect_tcp(host: String, port: u16) -> Result<u32, JsErrorBox> {
    let stream = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(io_error)?;
    let _ = stream.set_nodelay(true);
    Ok(insert_connection(Transport::Tcp(stream)))
}

#[op2(async(lazy), fast)]
async fn op_net_connect_unix(#[string] path: String, attempt: u32) -> Result<u32, JsErrorBox> {
    with_connect_cancel(attempt, connect_unix(path)).await
}

async fn connect_unix(path: String) -> Result<u32, JsErrorBox> {
    let stream = UnixStream::connect(&path).await.map_err(io_error)?;
    Ok(insert_connection(Transport::Stream(stream)))
}

#[derive(Clone, Copy)]
enum FdKind {
    Tcp,
    Stream,
    Pipe,
    Tty,
    File,
    Udp,
    Unknown,
}

impl FdKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Stream | Self::Pipe => "PIPE",
            Self::Tty => "TTY",
            Self::File => "FILE",
            Self::Udp => "UDP",
            Self::Unknown => "UNKNOWN",
        }
    }
}

fn fd_kind(fd: RawFd) -> FdKind {
    // SAFETY: `isatty` only inspects the descriptor and reports a bad one through its
    // return value.
    if unsafe { libc::isatty(fd) } == 1 {
        return FdKind::Tty;
    }
    // SAFETY: `libc::stat` is a plain C struct of integers, so an all-zero bit pattern
    // is a valid value.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `status` is a live `stat` the kernel writes into; `fstat` reports an
    // unusable descriptor through its return value instead of writing.
    if unsafe { libc::fstat(fd, &raw mut status) } != 0 {
        return FdKind::Unknown;
    }
    match status.st_mode & libc::S_IFMT {
        libc::S_IFREG => return FdKind::File,
        libc::S_IFIFO => return FdKind::Pipe,
        libc::S_IFSOCK => {}
        _ => return FdKind::Unknown,
    }
    let mut socket_type: libc::c_int = 0;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`size_of::<libc::c_int>()` is 4 on every supported platform"
    )]
    let mut length = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `socket_type` and `length` are live and of the size `length` describes;
    // `getsockopt` reports a bad descriptor through its return value.
    let got = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut socket_type).cast(),
            &raw mut length,
        )
    };
    if got != 0 {
        return FdKind::Unknown;
    }
    match (socket_type, socket_family(fd)) {
        (libc::SOCK_STREAM, libc::AF_INET | libc::AF_INET6) => FdKind::Tcp,
        (libc::SOCK_STREAM, libc::AF_UNIX) => FdKind::Stream,
        (libc::SOCK_DGRAM, libc::AF_INET | libc::AF_INET6) => FdKind::Udp,
        _ => FdKind::Unknown,
    }
}

fn socket_family(fd: RawFd) -> libc::c_int {
    // SAFETY: `libc::sockaddr_storage` is a plain C struct of integers, so an all-zero
    // bit pattern is a valid value.
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`sockaddr_storage` is 128 bytes on every supported platform"
    )]
    let mut length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    // SAFETY: `address` is a live `sockaddr_storage`, which is large enough for any
    // socket address the kernel fills in, and `length` describes its size exactly.
    let got = unsafe { libc::getsockname(fd, (&raw mut address).cast(), &raw mut length) };
    if got == 0 {
        libc::c_int::from(address.ss_family)
    } else {
        -1
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: `F_GETFL` only reads descriptor flags and reports a bad descriptor through
    // its return value.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::O_NONBLOCK != 0 {
        return Ok(());
    }
    // SAFETY: `O_NONBLOCK` is a flag bit valid for `F_SETFL`, and the call reports a bad
    // descriptor through its return value.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[op2(fast)]
fn op_net_from_fd(fd: i32) -> Result<u32, JsErrorBox> {
    let stream = match fd_kind(fd) {
        FdKind::Tcp => {
            set_nonblocking(fd).map_err(io_error)?;
            // SAFETY: `fd` was just verified to be a TCP socket; `from_raw_fd` takes over
            // ownership of the descriptor so it is closed exactly once.
            let stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
            Transport::Tcp(TcpStream::from_std(stream).map_err(io_error)?)
        }
        FdKind::Stream => {
            set_nonblocking(fd).map_err(io_error)?;
            // SAFETY: `fd` was just verified to be a Unix socket; `from_raw_fd` takes over
            // ownership of the descriptor so it is closed exactly once.
            let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
            Transport::Stream(UnixStream::from_std(stream).map_err(io_error)?)
        }
        FdKind::Pipe => {
            set_nonblocking(fd).map_err(io_error)?;
            // SAFETY: `fd` was just verified to be a pipe; `from_raw_fd` takes over
            // ownership of the descriptor so it is closed exactly once.
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            // SAFETY: `register` requires a non-blocking descriptor that stays open and
            // keeps its identity for the lifetime of the `AsyncFd`, which the `OwnedFd`
            // guarantees after the `set_nonblocking` above.
            Transport::Pipe(
                unsafe { AsyncFd::register(fd) }.map_err(|error| io_error(error.into()))?,
            )
        }
        kind => return Err(invalid_fd_type(kind)),
    };
    Ok(insert_connection(stream))
}

fn invalid_fd_type(kind: FdKind) -> JsErrorBox {
    JsErrorBox::new("Error", format!("ERR_INVALID_FD_TYPE\0{}", kind.name()))
}

fn read_cancelled() -> &'static Mutex<HashSet<u32>> {
    static CANCELLED: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    CANCELLED.get_or_init(|| Mutex::new(HashSet::new()))
}

#[op2(fast)]
fn op_net_cancel_read(id: u32) {
    read_cancelled()
        .lock()
        .expect("net read cancelled")
        .insert(id);
    let _ = READ_CANCELS.cancel(id);
}

#[op2(async(lazy), fast)]
#[buffer]
async fn op_net_read(id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let mut buffer = vec![0u8; 65536];
    let stream = stream_of(id)?;
    if read_cancelled()
        .lock()
        .expect("net read cancelled")
        .remove(&id)
    {
        return Err(cancelled("read aborted"));
    }
    let cancelled_signal = READ_CANCELS.register(id);
    let outcome = tokio::select! {
        result = stream.read(&mut buffer) => Some(result),
        _ = cancelled_signal => None,
    };
    READ_CANCELS.remove(id);
    read_cancelled()
        .lock()
        .expect("net read cancelled")
        .remove(&id);
    let read = outcome
        .ok_or_else(|| cancelled("read aborted"))?
        .map_err(io_error)?;
    buffer.truncate(read);
    Ok(buffer.into_boxed_slice())
}

#[op2(async(lazy), fast)]
async fn op_net_write(id: u32, #[buffer(copy)] data: Vec<u8>) -> Result<u32, JsErrorBox> {
    stream_of(id)?.write_all(&data).await.map_err(io_error)?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "op buffers come from V8 typed arrays, whose length stays below `u32::MAX`"
    )]
    let written = data.len() as u32;
    Ok(written)
}

#[op2(async(lazy), fast)]
async fn op_net_shutdown(id: u32) -> Result<(), JsErrorBox> {
    shutdown(id, libc::SHUT_WR)
}

#[op2(fast)]
fn op_net_close(id: u32) {
    let Some(connection) = table().lock().expect("connection table").remove(&id) else {
        return;
    };
    let _ = connection.stream.shutdown(libc::SHUT_RDWR);
}

#[op2]
#[string]
fn op_net_info(id: u32) -> String {
    table()
        .lock()
        .expect("connection table")
        .get(&id)
        .map_or_else(
            || "\0\0".to_owned(),
            |connection| {
                format!(
                    "{}\0{}\0{}",
                    connection.stream.kind(),
                    connection.local,
                    connection.remote
                )
            },
        )
}

#[op2(fast)]
fn op_net_set_no_delay(id: u32, no_delay: bool) -> Result<(), JsErrorBox> {
    stream_of(id)?.set_no_delay(no_delay).map_err(option_error)
}

#[op2(fast)]
fn op_net_set_keep_alive(id: u32, enable: bool, delay_seconds: u32) -> Result<(), JsErrorBox> {
    stream_of(id)?
        .set_keep_alive(enable, delay_seconds)
        .map_err(option_error)
}

fn shutdown(id: u32, how: i32) -> Result<(), JsErrorBox> {
    stream_of(id)?.shutdown(how).map_err(io_error)
}

#[derive(Debug)]
pub enum TakeStreamError {
    NotFound,
    Busy,
    NotTcp,
    Io(std::io::Error),
}

/// Takes the TCP stream out of the connection table, so TLS or HTTP/2 can take over
/// the socket.
///
/// # Errors
///
/// Returns [`TakeStreamError::NotFound`] for an unknown id, [`TakeStreamError::Busy`]
/// while another op still holds the connection, [`TakeStreamError::NotTcp`] for a Unix
/// socket or pipe, and [`TakeStreamError::Io`] when the std stream handoff fails.
///
/// # Panics
///
/// Panics if the connection table lock is poisoned.
pub fn take_stream(id: u32) -> Result<TcpStream, TakeStreamError> {
    let entry = table()
        .lock()
        .expect("connection table")
        .remove(&id)
        .ok_or(TakeStreamError::NotFound)?;
    let Connection {
        stream,
        local,
        remote,
    } = entry;
    let transport = match Arc::try_unwrap(stream) {
        Ok(transport) => transport,
        Err(stream) => {
            table().lock().expect("connection table").insert(
                id,
                Connection {
                    stream,
                    local,
                    remote,
                },
            );
            return Err(TakeStreamError::Busy);
        }
    };
    let Transport::Tcp(stream) = transport else {
        return Err(TakeStreamError::NotTcp);
    };
    let stream = stream.into_std().map_err(TakeStreamError::Io)?;
    TcpStream::from_std(stream).map_err(TakeStreamError::Io)
}

fn stream_of(id: u32) -> Result<Arc<Transport>, JsErrorBox> {
    table()
        .lock()
        .expect("connection table")
        .get(&id)
        .map(|connection| Arc::clone(&connection.stream))
        .ok_or_else(bad_connection)
}

enum Listener {
    Tcp {
        listener: tokio::net::TcpListener,
        local: String,
    },
    Unix {
        listener: tokio::net::UnixListener,
        local: String,
        path: Option<String>,
    },
}

impl Listener {
    fn local(&self) -> &str {
        match self {
            Self::Tcp { local, .. } | Self::Unix { local, .. } => local,
        }
    }
    const fn kind(&self) -> &'static str {
        match self {
            Self::Tcp { .. } => "tcp",
            Self::Unix { .. } => "unix",
        }
    }

    async fn accept(&self) -> Result<u32, JsErrorBox> {
        match self {
            Self::Tcp { listener, .. } => {
                let (stream, _) = listener.accept().await.map_err(io_error)?;
                let _ = stream.set_nodelay(true);
                Ok(insert_connection(Transport::Tcp(stream)))
            }
            Self::Unix { listener, .. } => {
                let (stream, _) = listener.accept().await.map_err(io_error)?;
                Ok(insert_connection(Transport::Stream(stream)))
            }
        }
    }
}

fn listeners() -> &'static Mutex<HashMap<u32, Arc<Listener>>> {
    static LISTENERS: OnceLock<Mutex<HashMap<u32, Arc<Listener>>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[op2(async(lazy), fast)]
async fn op_net_listen(#[string] host: String, port: u16) -> Result<u32, JsErrorBox> {
    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .map_err(io_error)?;
    let local = listener
        .local_addr()
        .map(|address| address.to_string())
        .unwrap_or_default();
    let id = LISTENERS.next();
    listeners()
        .lock()
        .expect("listener table")
        .insert(id, Arc::new(Listener::Tcp { listener, local }));
    Ok(id)
}

#[op2(async(lazy), fast)]
async fn op_net_listen_unix(#[string] path: String) -> Result<u32, JsErrorBox> {
    let listener = tokio::net::UnixListener::bind(&path).map_err(io_error)?;
    let id = LISTENERS.next();
    listeners().lock().expect("listener table").insert(
        id,
        Arc::new(Listener::Unix {
            listener,
            local: path.clone(),
            path: Some(path),
        }),
    );
    Ok(id)
}

#[op2(fast)]
fn op_net_listen_fd(fd: i32) -> Result<u32, JsErrorBox> {
    let kind = fd_kind(fd);
    if !matches!(kind, FdKind::Tcp | FdKind::Stream) {
        return Err(invalid_fd_type(kind));
    }
    set_nonblocking(fd).map_err(io_error)?;
    // SAFETY: `fd` was verified above to be a TCP or Unix socket, which is what
    // `listen(2)` requires; it reports a failure through its return value.
    if unsafe { libc::listen(fd, 511) } != 0 {
        return Err(io_error(io::Error::last_os_error()));
    }
    let listener = if matches!(kind, FdKind::Tcp) {
        // SAFETY: `fd` is a listening TCP socket; `from_raw_fd` takes over ownership of
        // the descriptor so it is closed exactly once.
        let listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
        let local = listener
            .local_addr()
            .map(|address| address.to_string())
            .unwrap_or_default();
        Listener::Tcp {
            listener: tokio::net::TcpListener::from_std(listener).map_err(io_error)?,
            local,
        }
    } else {
        // SAFETY: `fd` is a listening Unix socket; `from_raw_fd` takes over ownership of
        // the descriptor so it is closed exactly once.
        let std_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
        let listener = tokio::net::UnixListener::from_std(std_listener).map_err(io_error)?;
        let local = path_name(listener.local_addr());
        Listener::Unix {
            listener,
            local,
            path: None,
        }
    };
    let id = LISTENERS.next();
    listeners()
        .lock()
        .expect("listener table")
        .insert(id, Arc::new(listener));
    Ok(id)
}

#[op2(async(lazy), fast)]
async fn op_net_accept(id: u32) -> Result<u32, JsErrorBox> {
    let listener = listeners()
        .lock()
        .expect("listener table")
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(bad_connection)?;
    let cancelled_signal = ACCEPT_CANCELS.register(id);
    let result = tokio::select! {
        accepted = listener.accept() => accepted,
        _ = cancelled_signal => return Err(cancelled("accept aborted")),
    };
    ACCEPT_CANCELS.remove(id);
    result
}

#[op2(fast)]
fn op_net_close_listener(id: u32) {
    let listener = listeners().lock().expect("listener table").remove(&id);
    let _ = ACCEPT_CANCELS.cancel(id);
    if let Some(Listener::Unix {
        path: Some(path), ..
    }) = listener.as_deref()
    {
        let _ = std::fs::remove_file(path);
    }
}

#[op2]
#[string]
fn op_net_listen_info(id: u32) -> String {
    listeners()
        .lock()
        .expect("listener table")
        .get(&id)
        .map_or_else(String::new, |listener| {
            format!("{}\0{}", listener.kind(), listener.local())
        })
}
