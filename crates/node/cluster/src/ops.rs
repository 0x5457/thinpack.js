// `op2(async(lazy))` only accepts an `async fn`, so ops whose body has nothing
// to await still have to be async.
#![expect(
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "`op2(async(lazy))` rejects a non-async op fn"
)]

#[cfg(unix)]
mod unix {
    //! Ops backing `node:cluster`.
    //!
    //! The worker process itself — its pid, stdio pipes, wait and kill — lives in the shared
    //! `thinpack-child` table, the same one `node:child_process` uses. This module adds what
    //! makes a cluster worker a worker: the IPC channel and the descriptor the child inherits
    //! across `exec`, both keyed by the table entry's id.

    use std::collections::HashMap;
    use std::io;
    use std::mem::size_of;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::sync::{Arc, Mutex, OnceLock};

    use deno_core::{OpDecl, op2};
    use deno_error::JsErrorBox;
    use tokio::net::UnixStream;
    use tokio::sync::Mutex as AsyncMutex;

    use thinpack_child::{ChildError, InheritedFd, Spawn, Stdio};

    pub(crate) const OPS: &[OpDecl] = &[
        op_cluster_spawn(),
        op_cluster_channel_accept(),
        op_cluster_clear_env(),
        op_cluster_allocate_port(),
        op_cluster_channel_adopt(),
        op_cluster_channel_read(),
        op_cluster_channel_write(),
        op_cluster_channel_write_sync(),
        op_cluster_channel_close(),
    ];

    /// The descriptor the child side of the channel is moved to before `exec`.
    const CHANNEL_FD: RawFd = 3;

    struct ChannelEntry {
        stream: UnixStream,
        write: AsyncMutex<()>,
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

    /// A spawn failure in the errno form `UVException` is raised from.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "`ChildError` is a `map_err` sink; taking a reference would force a closure at every call site"
    )]
    fn spawn_error(error: ChildError) -> JsErrorBox {
        JsErrorBox::new("Error", error.to_uv_wire())
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
        let request = Spawn::parse(argv, env, cwd)
            .map_err(spawn_error)?
            .with_env("NODE_UNIQUE_ID", worker_id)
            .with_env("NODE_CHANNEL_FD", CHANNEL_FD);

        let (parent, child_end) = std::os::unix::net::UnixStream::pair().map_err(io_error)?;
        parent.set_nonblocking(true).map_err(io_error)?;
        let channel_fd = child_end.as_raw_fd();

        let stdio = if silent {
            Stdio::PIPED
        } else {
            Stdio::INHERITED
        };
        let inherited = InheritedFd {
            source: channel_fd,
            target: CHANNEL_FD,
        };
        let spawned =
            thinpack_child::spawn(&request, stdio, Some(inherited)).map_err(spawn_error)?;
        drop(child_end);

        let stream = UnixStream::from_std(parent).map_err(io_error)?;
        channels().lock().expect("channel table").insert(
            spawned.id,
            Arc::new(ChannelEntry {
                stream,
                write: AsyncMutex::new(()),
            }),
        );
        Ok(format!("{}\0{}", spawned.id, spawned.pid))
    }

    #[op2(async(lazy), fast)]
    async fn op_cluster_channel_accept(_id: u32) -> Result<(), JsErrorBox> {
        // On unix the channel is complete once `op_cluster_spawn` returns; the op exists so
        // the shared JS layer runs the same handshake on every platform (Windows connects
        // its loopback channel here).
        Ok(())
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
                write: AsyncMutex::new(()),
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
        // The frame header was already consumed, so an EOF here is a peer dying mid-frame
        // and is an error, not a clean channel close.
        if !fill_exact(&entry.stream, &mut payload)
            .await
            .map_err(io_error)?
        {
            return Err(io_error(io::Error::from(io::ErrorKind::UnexpectedEof)));
        }
        String::from_utf8(payload)
            .map_err(|_| cluster_error("EINVAL", "the channel frame is not UTF-8"))
    }

    #[op2(async(lazy), fast)]
    async fn op_cluster_channel_write(id: u32, #[string] frame: String) -> Result<(), JsErrorBox> {
        let entry = channel_of(id)?;
        let _guard = entry.write.lock().await;
        let mut bytes = Vec::with_capacity(4 + frame.len());
        bytes.extend_from_slice(&frame_length(&frame)?);
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
        let mut bytes = Vec::with_capacity(4 + frame.len());
        bytes.extend_from_slice(&frame_length(frame)?);
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

    /// The big-endian frame length that prefixes every channel message.
    fn frame_length(frame: &str) -> Result<[u8; 4], JsErrorBox> {
        let length = u32::try_from(frame.len())
            .map_err(|_| cluster_error("E2BIG", "the channel frame is larger than 4 GiB"))?;
        Ok(length.to_be_bytes())
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
}

#[cfg(unix)]
pub(crate) use unix::*;

#[cfg(windows)]
mod windows {
    //! Windows has no socketpair, so a worker's IPC channel is a loopback TCP connection:
    //! the primary listens on an ephemeral port, passes the port in the child's environment
    //! and accepts the connection during `op_cluster_channel_accept`. Everything about the
    //! process itself — pid, stdio pipes, wait, kill — is the shared `thinpack-child` table.

    use std::collections::HashMap;
    use std::io::{self, Write as _};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    use deno_core::{OpDecl, op2};
    use deno_error::JsErrorBox;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Mutex as AsyncMutex;

    use thinpack_child::{ChildError, Spawn, Stdio};

    /// How long the primary waits for a spawned worker to connect its channel before
    /// giving up on the handshake.
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

    pub(crate) const OPS: &[OpDecl] = &[
        op_cluster_spawn(),
        op_cluster_channel_accept(),
        op_cluster_clear_env(),
        op_cluster_allocate_port(),
        op_cluster_channel_adopt_port(),
        op_cluster_channel_read(),
        op_cluster_channel_write(),
        op_cluster_channel_write_sync(),
        op_cluster_channel_close(),
    ];

    struct ChannelEntry {
        read: AsyncMutex<tokio::net::tcp::OwnedReadHalf>,
        write_stream: AsyncMutex<tokio::net::tcp::OwnedWriteHalf>,
        blocking: std::net::TcpStream,
        write: AsyncMutex<()>,
    }

    fn channels() -> &'static Mutex<HashMap<u32, Arc<ChannelEntry>>> {
        static CHANNELS: OnceLock<Mutex<HashMap<u32, Arc<ChannelEntry>>>> = OnceLock::new();
        CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn listeners() -> &'static Mutex<HashMap<u32, tokio::net::TcpListener>> {
        static LISTENERS: OnceLock<Mutex<HashMap<u32, tokio::net::TcpListener>>> = OnceLock::new();
        LISTENERS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    static ENTRIES: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

    fn cluster_error(code: &str, detail: impl std::fmt::Display) -> JsErrorBox {
        JsErrorBox::new("Error", format!("{code}\0{detail}"))
    }

    fn io_error(error: io::Error) -> JsErrorBox {
        let errno = deno_core::uv_compat::io_error_to_uv(&error);
        JsErrorBox::new("Error", format!("{errno}\0{error}"))
    }

    /// A spawn failure in the errno form `UVException` is raised from.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "`ChildError` is a `map_err` sink; taking a reference would force a closure at every call site"
    )]
    fn spawn_error(error: ChildError) -> JsErrorBox {
        JsErrorBox::new("Error", error.to_uv_wire())
    }

    fn channel_of(id: u32) -> Result<Arc<ChannelEntry>, JsErrorBox> {
        channels()
            .lock()
            .expect("channel table")
            .get(&id)
            .map(Arc::clone)
            .ok_or_else(|| cluster_error("EPIPE", "cluster channel is closed"))
    }

    fn channel_entry(stream: std::net::TcpStream) -> Result<Arc<ChannelEntry>, JsErrorBox> {
        let blocking = stream.try_clone().map_err(io_error)?;
        stream.set_nonblocking(true).map_err(io_error)?;
        let stream = tokio::net::TcpStream::from_std(stream).map_err(io_error)?;
        let (read, write_stream) = stream.into_split();
        Ok(Arc::new(ChannelEntry {
            read: AsyncMutex::new(read),
            write_stream: AsyncMutex::new(write_stream),
            blocking,
            write: AsyncMutex::new(()),
        }))
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
        // The child gets the port in its environment and connects during its bootstrap; the
        // primary completes the handshake in `op_cluster_channel_accept`.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(io_error)?;
        listener.set_nonblocking(true).map_err(io_error)?;
        let port = listener.local_addr().map_err(io_error)?.port();
        let listener = tokio::net::TcpListener::from_std(listener).map_err(io_error)?;

        let request = Spawn::parse(argv, env, cwd)
            .map_err(spawn_error)?
            .with_env("NODE_UNIQUE_ID", worker_id)
            .with_env("NODE_CHANNEL_PORT", port);
        let stdio = if silent {
            Stdio::PIPED
        } else {
            Stdio::INHERITED
        };
        let spawned = thinpack_child::spawn(&request, stdio, None).map_err(spawn_error)?;
        listeners()
            .lock()
            .expect("listener table")
            .insert(spawned.id, listener);
        Ok(format!("{}\0{}", spawned.id, spawned.pid))
    }

    #[op2(async(lazy), fast)]
    async fn op_cluster_channel_accept(id: u32) -> Result<(), JsErrorBox> {
        let listener = listeners().lock().expect("listener table").remove(&id);
        let Some(listener) = listener else {
            return Err(cluster_error("EINVAL", "the channel handshake already ran"));
        };
        // The handshake runs asynchronously so a worker that never connects never blocks
        // the event loop with sleep polling; `cluster.fork` has already returned by now.
        let accepted = tokio::time::timeout(HANDSHAKE_TIMEOUT, listener.accept()).await;
        let stream = match accepted {
            Ok(Ok((stream, _))) => stream.into_std().map_err(io_error)?,
            Ok(Err(error)) => return Err(io_error(error)),
            Err(_) => {
                // A worker that never connected is useless; any signal terminates it on
                // Windows, and 9 is the `SIGKILL` the unix path would use.
                let _ = thinpack_child::kill(id, 9);
                return Err(cluster_error(
                    "ETIMEDOUT",
                    "the cluster worker did not connect",
                ));
            }
        };
        let channel = channel_entry(stream)?;
        channels()
            .lock()
            .expect("channel table")
            .insert(id, channel);
        Ok(())
    }

    #[op2(fast)]
    fn op_cluster_clear_env() {
        // SAFETY: called once during worker startup, before user code can spawn.
        unsafe {
            std::env::remove_var("NODE_UNIQUE_ID");
            std::env::remove_var("NODE_CHANNEL_PORT");
            std::env::remove_var("NODE_CHANNEL_FD");
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
    fn op_cluster_channel_adopt_port(port: u16) -> Result<u32, JsErrorBox> {
        let stream = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(io_error)?;
        let channel = channel_entry(stream)?;
        let id = ENTRIES.next();
        channels()
            .lock()
            .expect("channel table")
            .insert(id, channel);
        Ok(id)
    }

    async fn fill_exact(
        stream: &mut tokio::net::tcp::OwnedReadHalf,
        buffer: &mut [u8],
    ) -> io::Result<bool> {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = stream.read(&mut buffer[filled..]).await?;
            if read == 0 {
                if filled == 0 {
                    return Ok(false);
                }
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            filled += read;
        }
        Ok(true)
    }

    #[op2(async(lazy), fast)]
    #[string]
    async fn op_cluster_channel_read(id: u32) -> Result<String, JsErrorBox> {
        let entry = channel_of(id)?;
        let mut stream = entry.read.lock().await;
        let mut header = [0u8; 4];
        if !fill_exact(&mut stream, &mut header)
            .await
            .map_err(io_error)?
        {
            return Ok(String::new());
        }
        let length =
            usize::try_from(u32::from_be_bytes(header)).expect("a u32 frame length fits in usize");
        let mut payload = vec![0u8; length];
        // The frame header was already consumed, so an EOF here is a peer dying mid-frame
        // and is an error, not a clean channel close.
        if !fill_exact(&mut stream, &mut payload)
            .await
            .map_err(io_error)?
        {
            return Err(io_error(io::Error::from(io::ErrorKind::UnexpectedEof)));
        }
        String::from_utf8(payload)
            .map_err(|_| cluster_error("EINVAL", "the channel frame is not UTF-8"))
    }

    #[op2(async(lazy), fast)]
    async fn op_cluster_channel_write(id: u32, #[string] frame: String) -> Result<(), JsErrorBox> {
        let entry = channel_of(id)?;
        let _guard = entry.write.lock().await;
        let mut bytes = Vec::with_capacity(4 + frame.len());
        bytes.extend_from_slice(&frame_length(&frame)?);
        bytes.extend_from_slice(frame.as_bytes());
        entry
            .write_stream
            .lock()
            .await
            .write_all(&bytes)
            .await
            .map_err(io_error)
    }

    #[op2(fast)]
    fn op_cluster_channel_write_sync(id: u32, #[string] frame: &str) -> Result<(), JsErrorBox> {
        let entry = channel_of(id)?;
        let mut bytes = Vec::with_capacity(4 + frame.len());
        bytes.extend_from_slice(&frame_length(frame)?);
        bytes.extend_from_slice(frame.as_bytes());
        let mut stream = &entry.blocking;
        stream.write_all(&bytes).map_err(io_error)
    }

    #[op2(fast)]
    fn op_cluster_channel_close(id: u32) {
        if let Some(entry) = channels().lock().expect("channel table").remove(&id) {
            let _ = entry.blocking.shutdown(std::net::Shutdown::Both);
        }
    }

    /// The big-endian frame length that prefixes every channel message.
    fn frame_length(frame: &str) -> Result<[u8; 4], JsErrorBox> {
        let length = u32::try_from(frame.len())
            .map_err(|_| cluster_error("E2BIG", "the channel frame is larger than 4 GiB"))?;
        Ok(length.to_be_bytes())
    }
}

#[cfg(windows)]
pub(crate) use windows::*;
