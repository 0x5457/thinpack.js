//! The IPC channel shared by `node:child_process` and `node:cluster`.
//!
//! Node builds both on the same primitive: a pipe (a socketpair on unix, a named pipe on
//! Windows) whose messages are newline-delimited JSON. The shared layer owns that pipe and
//! its table, keyed by a channel id that is independent of the spawned-process table entry:
//! one process can be a channel child and the parent of other channel children at the same
//! time, and the two sides must not share an id namespace.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use deno_error::JsErrorBox;

use crate::{ChildError, Spawned};

/// A spawned child and the parent end of its IPC channel.
#[derive(Debug)]
pub struct SpawnedChannel {
    pub spawned: Spawned,
    /// The channel id the parent-side ops address.
    pub channel: u32,
    /// The parent end's descriptor on unix; `-1` where the channel has no descriptor.
    pub fd: i32,
}

fn io_error(error: io::Error) -> ChildError {
    // Node surfaces a broken channel as EPIPE/ECONNRESET with `syscall: 'write'`; keep the
    // symbolic code so the shared JavaScript layer can report the same shape.
    let code = match error.kind() {
        io::ErrorKind::BrokenPipe => "EPIPE",
        io::ErrorKind::ConnectionReset => "ECONNRESET",
        io::ErrorKind::NotConnected => "ENOTCONN",
        _ => "EIO",
    };
    ChildError::io(code, error)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "`ChildError` is a `map_err` sink; taking a reference would force a closure at every call site"
)]
fn wire_error(error: ChildError) -> JsErrorBox {
    JsErrorBox::new("Error", error.to_wire())
}

fn channel_of<T>(
    channels: &'static Mutex<HashMap<u32, Arc<T>>>,
    id: u32,
) -> Result<Arc<T>, ChildError> {
    channels
        .lock()
        .expect("channel table")
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| ChildError::new("EPIPE", "IPC channel is closed"))
}

fn frame_bytes(frame: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(frame.len() + 1);
    bytes.extend_from_slice(frame.as_bytes());
    bytes.push(b'\n');
    bytes
}

fn utf8_frame(frame: Vec<u8>) -> Result<String, ChildError> {
    String::from_utf8(frame)
        .map_err(|_| ChildError::new("EINVAL", "the channel frame is not UTF-8"))
}

#[cfg(unix)]
mod unix {
    //! The unix half: a stream socketpair, one end inherited by the child across `exec`
    //! and the other read and written by the parent-side ops.
    #![expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "`op2(async(lazy))` rejects a non-async op fn"
    )]

    use std::collections::HashMap;
    use std::io;
    use std::mem::size_of;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::sync::{Arc, Mutex, OnceLock};

    use deno_core::{OpDecl, op2};
    use deno_error::JsErrorBox;
    use tokio::net::UnixStream;

    use super::{SpawnedChannel, channel_of, frame_bytes, io_error, wire_error};
    use crate::{ChildError, InheritedFd, Spawn, Spawned, Stdio};

    struct ChannelEntry {
        stream: UnixStream,
        read: tokio::sync::Mutex<Vec<u8>>,
        write: tokio::sync::Mutex<()>,
    }

    fn channels() -> &'static Mutex<HashMap<u32, Arc<ChannelEntry>>> {
        static CHANNELS: OnceLock<Mutex<HashMap<u32, Arc<ChannelEntry>>>> = OnceLock::new();
        CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    static ENTRIES: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

    fn insert(stream: UnixStream) -> u32 {
        let id = ENTRIES.next();
        channels().lock().expect("channel table").insert(
            id,
            Arc::new(ChannelEntry {
                stream,
                read: tokio::sync::Mutex::new(Vec::new()),
                write: tokio::sync::Mutex::new(()),
            }),
        );
        id
    }

    /// Spawns a child with `target` connected to a new channel.
    ///
    /// # Errors
    ///
    /// Returns the spawn failure, or the socketpair failure when one cannot be created.
    pub fn spawn_with_channel(
        request: Spawn,
        stdio: Stdio,
        target: i32,
    ) -> Result<SpawnedChannel, ChildError> {
        let (parent, child_end) = std::os::unix::net::UnixStream::pair().map_err(io_error)?;
        parent.set_nonblocking(true).map_err(io_error)?;
        let source = child_end.as_raw_fd();
        let parent_fd = parent.as_raw_fd();

        let request = request
            .with_env("NODE_CHANNEL_FD", target)
            .with_env("NODE_CHANNEL_SERIALIZATION_MODE", "json");
        let inherited = InheritedFd { source, target };
        let spawned: Spawned = crate::spawn(&request, stdio, Some(inherited))?;
        drop(child_end);

        let stream = UnixStream::from_std(parent).map_err(io_error)?;
        Ok(SpawnedChannel {
            spawned,
            channel: insert(stream),
            fd: parent_fd,
        })
    }

    /// Takes ownership of a channel descriptor inherited from a parent.
    ///
    /// # Errors
    ///
    /// Returns the descriptor validation failure from `fcntl` or `getsockopt`.
    pub(crate) fn adopt(fd: i32) -> Result<u32, ChildError> {
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
            return Err(ChildError::new(
                "ENOTSOCK",
                "the channel descriptor is not a stream socket",
            ));
        }

        // SAFETY: the descriptor is open and is a stream socket, and ownership moves into the
        // `UnixStream` from here on.
        let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
        stream.set_nonblocking(true).map_err(io_error)?;
        clear_channel_env();
        Ok(insert(UnixStream::from_std(stream).map_err(io_error)?))
    }

    fn clear_channel_env() {
        // SAFETY: a channel child calls this once during startup, before user code can spawn
        // children. These variables belong to the bootstrap and are not read afterward.
        unsafe {
            std::env::remove_var("NODE_CHANNEL_FD");
            std::env::remove_var("NODE_CHANNEL_PORT");
            std::env::remove_var("NODE_CHANNEL_SERIALIZATION_MODE");
        }
    }

    async fn read_frame(entry: &ChannelEntry) -> Result<String, ChildError> {
        let mut pending = entry.read.lock().await;
        loop {
            if let Some(index) = pending.iter().position(|&byte| byte == b'\n') {
                let mut frame: Vec<u8> = pending.drain(..=index).collect();
                frame.pop();
                return super::utf8_frame(frame);
            }
            let mut chunk = [0u8; 4096];
            entry.stream.readable().await.map_err(io_error)?;
            match entry.stream.try_read(&mut chunk) {
                Ok(0) => {
                    if pending.is_empty() {
                        return Ok(String::new());
                    }
                    return Err(io_error(io::ErrorKind::UnexpectedEof.into()));
                }
                Ok(count) => pending.extend_from_slice(&chunk[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(io_error(error)),
            }
        }
    }

    async fn write_frame(entry: &ChannelEntry, frame: &str) -> Result<(), ChildError> {
        let _guard = entry.write.lock().await;
        write_all(&entry.stream, &frame_bytes(frame)).await
    }

    /// Used once at startup for cluster's `online` message, before user code can write.
    fn write_frame_sync(entry: &ChannelEntry, frame: &str) -> Result<(), ChildError> {
        let bytes = frame_bytes(frame);
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
                return Err(ChildError::new("EPIPE", "channel closed"));
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

    async fn write_all(stream: &UnixStream, bytes: &[u8]) -> Result<(), ChildError> {
        let mut written = 0;
        while written < bytes.len() {
            stream.writable().await.map_err(io_error)?;
            match stream.try_write(&bytes[written..]) {
                Ok(0) => return Err(io_error(io::ErrorKind::BrokenPipe.into())),
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(())
    }

    pub(crate) const OPS: &[OpDecl] = &[
        op_child_channel_accept(),
        op_child_channel_adopt(),
        op_child_channel_read(),
        op_child_channel_write(),
        op_child_channel_write_sync(),
        op_child_channel_close(),
    ];

    #[op2(async(lazy), fast)]
    async fn op_child_channel_accept(_channel: u32, _child: u32) -> Result<(), JsErrorBox> {
        Ok(())
    }

    #[op2(fast)]
    fn op_child_channel_adopt(fd: i32) -> Result<u32, JsErrorBox> {
        adopt(fd).map_err(wire_error)
    }

    #[op2(async(lazy), fast)]
    #[string]
    async fn op_child_channel_read(id: u32) -> Result<String, JsErrorBox> {
        let entry = channel_of(channels(), id).map_err(wire_error)?;
        read_frame(&entry).await.map_err(wire_error)
    }

    #[op2(async(lazy), fast)]
    async fn op_child_channel_write(id: u32, #[string] frame: String) -> Result<(), JsErrorBox> {
        let entry = channel_of(channels(), id).map_err(wire_error)?;
        write_frame(&entry, &frame).await.map_err(wire_error)
    }

    #[op2(fast)]
    fn op_child_channel_write_sync(id: u32, #[string] frame: &str) -> Result<(), JsErrorBox> {
        let entry = channel_of(channels(), id).map_err(wire_error)?;
        write_frame_sync(&entry, frame).map_err(wire_error)
    }

    #[op2(fast)]
    fn op_child_channel_close(id: u32) {
        let Some(entry) = channels().lock().expect("channel table").remove(&id) else {
            return;
        };
        // SAFETY: `shutdown` only reads the descriptor owned by `entry.stream`; it wakes a
        // pending read so the JS pump observes the close immediately.
        unsafe { libc::shutdown(entry.stream.as_raw_fd(), libc::SHUT_RDWR) };
    }
}

#[cfg(windows)]
mod windows {
    //! Windows has no socketpair, so a channel is a loopback TCP connection: the parent
    //! listens on an ephemeral port, passes the port in the child's environment and accepts
    //! the connection when the parent-side read loop starts.

    use std::collections::HashMap;
    use std::io::{self, Write as _};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    use deno_core::{OpDecl, op2};
    use deno_error::JsErrorBox;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Mutex as AsyncMutex;

    use super::{SpawnedChannel, channel_of, frame_bytes, io_error, wire_error};
    use crate::{ChildError, Spawn, Stdio};

    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

    struct ChannelEntry {
        read: AsyncMutex<(tokio::net::tcp::OwnedReadHalf, Vec<u8>)>,
        write_stream: AsyncMutex<tokio::net::tcp::OwnedWriteHalf>,
        blocking: std::net::TcpStream,
        write: AsyncMutex<()>,
    }

    struct Listener {
        listener: tokio::net::TcpListener,
        child: u32,
    }

    fn channels() -> &'static Mutex<HashMap<u32, Arc<ChannelEntry>>> {
        static CHANNELS: OnceLock<Mutex<HashMap<u32, Arc<ChannelEntry>>>> = OnceLock::new();
        CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn listeners() -> &'static Mutex<HashMap<u32, Listener>> {
        static LISTENERS: OnceLock<Mutex<HashMap<u32, Listener>>> = OnceLock::new();
        LISTENERS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    static ENTRIES: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

    fn channel_entry(stream: std::net::TcpStream) -> Result<Arc<ChannelEntry>, ChildError> {
        let blocking = stream.try_clone().map_err(io_error)?;
        stream.set_nonblocking(true).map_err(io_error)?;
        let stream = tokio::net::TcpStream::from_std(stream).map_err(io_error)?;
        let (read, write_stream) = stream.into_split();
        Ok(Arc::new(ChannelEntry {
            read: AsyncMutex::new((read, Vec::new())),
            write_stream: AsyncMutex::new(write_stream),
            blocking,
            write: AsyncMutex::new(()),
        }))
    }

    fn insert(stream: std::net::TcpStream) -> Result<u32, ChildError> {
        let id = ENTRIES.next();
        channels()
            .lock()
            .expect("channel table")
            .insert(id, channel_entry(stream)?);
        Ok(id)
    }

    /// Spawns a child that connects back to a new loopback channel.
    ///
    /// # Errors
    ///
    /// Returns the listener or spawn failure.
    pub fn spawn_with_channel(
        request: Spawn,
        stdio: Stdio,
        _target: i32,
    ) -> Result<SpawnedChannel, ChildError> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(io_error)?;
        listener.set_nonblocking(true).map_err(io_error)?;
        let port = listener.local_addr().map_err(io_error)?.port();
        let listener = tokio::net::TcpListener::from_std(listener).map_err(io_error)?;
        let channel = ENTRIES.next();

        let request = request
            .with_env("NODE_CHANNEL_PORT", port)
            .with_env("NODE_CHANNEL_SERIALIZATION_MODE", "json");
        let spawned = crate::spawn(&request, stdio, None)?;
        listeners().lock().expect("listener table").insert(
            channel,
            Listener {
                listener,
                child: spawned.id,
            },
        );
        Ok(SpawnedChannel {
            spawned,
            channel,
            fd: -1,
        })
    }

    /// Accepts the child's loopback connection.
    ///
    /// # Errors
    ///
    /// Returns the accept failure, or `ETIMEDOUT` after killing a child that never connected.
    pub(crate) async fn accept(channel: u32, _child: u32) -> Result<(), ChildError> {
        let Some(Listener {
            listener,
            child: recorded,
        }) = listeners().lock().expect("listener table").remove(&channel)
        else {
            return Err(ChildError::new(
                "EINVAL",
                "the channel handshake already ran",
            ));
        };
        let accepted = tokio::time::timeout(HANDSHAKE_TIMEOUT, listener.accept()).await;
        let (stream, _) = match accepted {
            Ok(Ok(pair)) => pair,
            Ok(Err(error)) => return Err(io_error(error)),
            Err(_) => {
                let _ = crate::kill(recorded, 9);
                return Err(ChildError::new(
                    "ETIMEDOUT",
                    "the IPC child did not connect",
                ));
            }
        };
        channels().lock().expect("channel table").insert(
            channel,
            channel_entry(stream.into_std().map_err(io_error)?)?,
        );
        Ok(())
    }

    /// Connects to the parent's loopback listener.
    ///
    /// # Errors
    ///
    /// Returns the connection failure.
    pub(crate) fn adopt_port(port: u16) -> Result<u32, ChildError> {
        let stream = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(io_error)?;
        clear_channel_env();
        insert(stream)
    }

    fn clear_channel_env() {
        // SAFETY: a channel child calls this once during startup, before user code can spawn
        // children. These variables belong to the bootstrap and are not read afterward.
        unsafe {
            std::env::remove_var("NODE_CHANNEL_FD");
            std::env::remove_var("NODE_CHANNEL_PORT");
            std::env::remove_var("NODE_CHANNEL_SERIALIZATION_MODE");
        }
    }

    async fn read_frame(entry: &ChannelEntry) -> Result<String, ChildError> {
        let mut guard = entry.read.lock().await;
        let (stream, pending) = &mut *guard;
        loop {
            if let Some(index) = pending.iter().position(|&byte| byte == b'\n') {
                let mut frame: Vec<u8> = pending.drain(..=index).collect();
                frame.pop();
                return super::utf8_frame(frame);
            }
            let mut chunk = [0u8; 4096];
            let count = stream.read(&mut chunk).await.map_err(io_error)?;
            if count == 0 {
                if pending.is_empty() {
                    return Ok(String::new());
                }
                return Err(io_error(io::ErrorKind::UnexpectedEof.into()));
            }
            pending.extend_from_slice(&chunk[..count]);
        }
    }

    async fn write_frame(entry: &ChannelEntry, frame: &str) -> Result<(), ChildError> {
        let _guard = entry.write.lock().await;
        entry
            .write_stream
            .lock()
            .await
            .write_all(&frame_bytes(frame))
            .await
            .map_err(io_error)
    }

    fn write_frame_sync(entry: &ChannelEntry, frame: &str) -> Result<(), ChildError> {
        let mut stream = &entry.blocking;
        stream.write_all(&frame_bytes(frame)).map_err(io_error)
    }

    pub(crate) const OPS: &[OpDecl] = &[
        op_child_channel_accept(),
        op_child_channel_adopt_port(),
        op_child_channel_read(),
        op_child_channel_write(),
        op_child_channel_write_sync(),
        op_child_channel_close(),
    ];

    #[op2(async(lazy), fast)]
    async fn op_child_channel_accept(channel: u32, child: u32) -> Result<(), JsErrorBox> {
        accept(channel, child).await.map_err(wire_error)
    }

    #[op2(fast)]
    fn op_child_channel_adopt_port(port: u16) -> Result<u32, JsErrorBox> {
        adopt_port(port).map_err(wire_error)
    }

    #[op2(async(lazy), fast)]
    #[string]
    async fn op_child_channel_read(id: u32) -> Result<String, JsErrorBox> {
        let entry = channel_of(channels(), id).map_err(wire_error)?;
        read_frame(&entry).await.map_err(wire_error)
    }

    #[op2(async(lazy), fast)]
    async fn op_child_channel_write(id: u32, #[string] frame: String) -> Result<(), JsErrorBox> {
        let entry = channel_of(channels(), id).map_err(wire_error)?;
        write_frame(&entry, &frame).await.map_err(wire_error)
    }

    #[op2(fast)]
    fn op_child_channel_write_sync(id: u32, #[string] frame: &str) -> Result<(), JsErrorBox> {
        let entry = channel_of(channels(), id).map_err(wire_error)?;
        write_frame_sync(&entry, frame).map_err(wire_error)
    }

    #[op2(fast)]
    fn op_child_channel_close(id: u32) {
        if let Some(entry) = channels().lock().expect("channel table").remove(&id) {
            let _ = entry.blocking.shutdown(std::net::Shutdown::Both);
        }
    }
}

#[cfg(unix)]
pub(crate) use unix::OPS;
#[cfg(unix)]
pub use unix::spawn_with_channel;

#[cfg(windows)]
pub(crate) use windows::OPS;
#[cfg(windows)]
pub use windows::spawn_with_channel;
