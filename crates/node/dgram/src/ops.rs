//! UDP datagram sockets for `node:dgram`.
//!
//! `dgram.js` owns Node's public API: argument parsing, lookup, the receive pump, and
//! the wait that lets `send()` implicitly bind. This layer is the libuv-shaped core:
//! a table of bound sockets addressed by an id, one pending `recv_from` per bound socket,
//! and the socket options `dgram.Socket` exposes. Addresses are always numeric here;
//! `op_dgram_lookup` is the only name-resolution entry point, which keeps a custom
//! `lookup` option in JavaScript where Node keeps it.

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use deno_core::convert::Uint8Array;
use deno_core::{OpDecl, ToV8, op2};
use deno_error::JsErrorBox;
use socket2::{Domain, Protocol, SockRef, Socket, Type};
use tokio::net::UdpSocket;

const EBADF: i32 = 9;
const EINVAL: i32 = 22;

/// Receive buffer large enough for any UDP payload (the IPv6 maximum is 65527 bytes).
const MAX_DATAGRAM: usize = 65536;

/// `op_dgram_bind` flags: the requested address family and the bind options
/// Node's `createSocket` accepts.
const FLAG_UDP6: u8 = 1 << 0;
const FLAG_REUSE_ADDR: u8 = 1 << 1;
const FLAG_REUSE_PORT: u8 = 1 << 2;
const FLAG_IPV6_ONLY: u8 = 1 << 3;

pub(crate) const OPS: &[OpDecl] = &[
    op_dgram_lookup(),
    op_dgram_bind(),
    op_dgram_connect(),
    op_dgram_disconnect(),
    op_dgram_send(),
    op_dgram_recv(),
    op_dgram_cancel_recv(),
    op_dgram_close(),
    op_dgram_local_info(),
    op_dgram_remote_info(),
    op_dgram_set_broadcast(),
    op_dgram_set_ttl(),
    op_dgram_set_multicast_ttl(),
    op_dgram_set_multicast_loopback(),
    op_dgram_set_multicast_interface(),
    op_dgram_add_membership(),
    op_dgram_drop_membership(),
    op_dgram_add_source_membership(),
    op_dgram_drop_source_membership(),
    op_dgram_get_buffer_size(),
    op_dgram_set_buffer_size(),
];

fn sockets() -> MutexGuard<'static, HashMap<u32, Arc<UdpSocket>>> {
    static SOCKETS: OnceLock<Mutex<HashMap<u32, Arc<UdpSocket>>>> = OnceLock::new();
    SOCKETS
        .get_or_init(Default::default)
        .lock()
        .expect("dgram socket table")
}

static IDS: thinpack_core::IdCounter = thinpack_core::IdCounter::new();
static RECV_CANCELS: thinpack_core::CancelTable = thinpack_core::CancelTable::new();

/// `op_dgram_recv` is lazy: its body may start after `close()` has already cancelled the
/// descriptor. The tombstone keeps that cancellation from being lost and leaving a pending
/// receive that keeps the event loop alive.
fn cancelled() -> MutexGuard<'static, HashSet<u32>> {
    static CANCELLED: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    CANCELLED
        .get_or_init(Default::default)
        .lock()
        .expect("dgram cancelled receives")
}

fn socket_of(id: u32) -> Result<Arc<UdpSocket>, JsErrorBox> {
    sockets()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| errno(EBADF))
}

fn errno(code: i32) -> JsErrorBox {
    JsErrorBox::new("Error", (-code).to_string())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "`io::Error` is a `map_err` sink: taking a reference would only force a closure at every call site"
)]
fn io_error(error: io::Error) -> JsErrorBox {
    // libuv reports the raw OS errno on Unix, which is what Node's messages and
    // tests expect (for example `send EMSGSIZE` instead of the collapsed EINVAL
    // that `ErrorKind::InvalidInput` would produce).
    #[cfg(unix)]
    let errno = error.raw_os_error().map_or(-EINVAL, |code| -code);
    #[cfg(windows)]
    let errno = deno_core::uv_compat::io_error_to_uv(&error);
    JsErrorBox::new("Error", errno.to_string())
}

fn invalid() -> JsErrorBox {
    errno(EINVAL)
}

fn cancelled_error() -> JsErrorBox {
    JsErrorBox::new("Error", "ECANCELED\0receive aborted")
}

fn family_name(address: &IpAddr) -> String {
    if address.is_ipv4() {
        "IPv4".to_owned()
    } else {
        "IPv6".to_owned()
    }
}

#[derive(ToV8)]
struct SocketInfo {
    address: String,
    family: String,
    port: u16,
}

#[derive(ToV8)]
struct Datagram {
    data: Uint8Array,
    address: String,
    port: u16,
}

#[op2(async(lazy), fast)]
#[string]
async fn op_dgram_lookup(#[string] host: String, family: u8) -> Result<String, JsErrorBox> {
    let mut addresses = tokio::net::lookup_host((host.as_str(), 0))
        .await
        .map_err(|_| name_error())?;
    addresses
        .find(|address| {
            if family == 4 {
                address.is_ipv4()
            } else {
                address.is_ipv6()
            }
        })
        .map(|address| address.ip().to_string())
        .ok_or_else(name_error)
}

fn name_error() -> JsErrorBox {
    JsErrorBox::new("Error", "ENOTFOUND")
}

#[op2(fast)]
fn op_dgram_bind(#[string] address: &str, #[smi] port: u16, flags: u8) -> Result<u32, JsErrorBox> {
    let ip: IpAddr = address.parse().map_err(|_| invalid())?;
    if ip.is_ipv6() != (flags & FLAG_UDP6 != 0) {
        return Err(invalid());
    }
    let domain = if flags & FLAG_UDP6 != 0 {
        Domain::IPV6
    } else {
        Domain::IPV4
    };
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP)).map_err(io_error)?;
    socket
        .set_reuse_address(flags & FLAG_REUSE_ADDR != 0)
        .map_err(io_error)?;
    if flags & FLAG_REUSE_PORT != 0 {
        set_reuse_port(&socket).map_err(io_error)?;
    }
    if ip.is_ipv6() && flags & FLAG_IPV6_ONLY != 0 {
        socket.set_only_v6(true).map_err(io_error)?;
    }
    socket
        .bind(&SocketAddr::new(ip, port).into())
        .map_err(io_error)?;
    socket.set_nonblocking(true).map_err(io_error)?;

    let socket: std::net::UdpSocket = socket.into();
    let socket = UdpSocket::from_std(socket).map_err(io_error)?;
    let id = IDS.next();
    sockets().insert(id, Arc::new(socket));
    Ok(id)
}

#[cfg(unix)]
/// The `socklen_t` length of a socket-option structure known at compile time.
const fn socklen_of<T>() -> libc::socklen_t {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "every socket-option structure is far smaller than socklen_t's range"
    )]
    let length = std::mem::size_of::<T>() as libc::socklen_t;
    length
}

#[cfg(unix)]
fn set_reuse_port(socket: &Socket) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let enable: libc::c_int = 1;
    // SAFETY: `fd` is an open datagram socket owned by `socket`, and `enable` is a
    // correctly sized `c_int` for SO_REUSEPORT.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_REUSEPORT,
            (&raw const enable).cast(),
            socklen_of::<libc::c_int>(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn set_reuse_port(_socket: &Socket) -> io::Result<()> {
    // Windows has no SO_REUSEPORT equivalent; Node's libuv does not select one there.
    Ok(())
}

#[op2(async(lazy), fast)]
async fn op_dgram_connect(
    id: u32,
    #[string] address: String,
    #[smi] port: u16,
) -> Result<(), JsErrorBox> {
    let socket = socket_of(id)?;
    let ip: IpAddr = address.parse().map_err(|_| invalid())?;
    socket
        .connect(SocketAddr::new(ip, port))
        .await
        .map_err(io_error)
}

#[op2(fast)]
fn op_dgram_disconnect(id: u32) -> Result<(), JsErrorBox> {
    let socket = socket_of(id)?;
    disconnect(&socket).map_err(io_error)
}

#[cfg(unix)]
fn disconnect(socket: &UdpSocket) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // `connect` to AF_UNSPEC is how libuv's uv__udp_disconnect drops a UDP peer.
    // SAFETY: `socket` owns an open descriptor; an all-zero `sockaddr_storage` with
    // only `ss_family = AF_UNSPEC` is the documented disconnect form.
    let address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    // SAFETY: `address` is a valid `sockaddr_storage` and the length describes it.
    let result = unsafe {
        libc::connect(
            socket.as_raw_fd(),
            (&raw const address).cast(),
            socklen_of::<libc::sockaddr_storage>(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn disconnect(socket: &UdpSocket) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;

    use windows_sys::Win32::Networking::WinSock::{AF_UNSPEC, SOCKADDR, connect};

    let address = SOCKADDR {
        sa_family: AF_UNSPEC as u16,
        sa_data: [0; 14],
    };
    // SAFETY: the socket is open, and `address` is a correctly sized `sockaddr`
    // with AF_UNSPEC, which Winsock treats as a disconnect.
    let result = unsafe {
        connect(
            socket.as_raw_socket() as usize,
            &raw const address,
            std::mem::size_of::<SOCKADDR>() as i32,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[op2(async(lazy))]
async fn op_dgram_send(
    id: u32,
    #[buffer(copy)] data: Vec<u8>,
    #[string] address: Option<String>,
    #[smi] port: u16,
) -> Result<u32, JsErrorBox> {
    let socket = socket_of(id)?;
    let sent = match address {
        Some(address) => {
            let ip: IpAddr = address.parse().map_err(|_| invalid())?;
            socket
                .send_to(&data, SocketAddr::new(ip, port))
                .await
                .map_err(io_error)?
        }
        None => socket.send(&data).await.map_err(io_error)?,
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a datagram is at most `MAX_DATAGRAM` bytes"
    )]
    Ok(sent as u32)
}

#[op2(fast)]
fn op_dgram_cancel_recv(id: u32) {
    cancelled().insert(id);
    let _ = RECV_CANCELS.cancel(id);
}

#[op2(async(lazy), fast)]
async fn op_dgram_recv(id: u32) -> Result<Datagram, JsErrorBox> {
    let socket = socket_of(id)?;
    if cancelled().remove(&id) {
        return Err(cancelled_error());
    }
    let cancelled_signal = RECV_CANCELS.register(id);
    let mut buffer = vec![0u8; MAX_DATAGRAM];
    let outcome = tokio::select! {
        result = socket.recv_from(&mut buffer) => Some(result),
        _ = cancelled_signal => None,
    };
    RECV_CANCELS.remove(id);
    cancelled().remove(&id);

    let (read, remote) = match outcome {
        Some(result) => result.map_err(io_error)?,
        None => return Err(cancelled_error()),
    };
    buffer.truncate(read);
    Ok(Datagram {
        data: Uint8Array::from(buffer),
        address: remote.ip().to_string(),
        port: remote.port(),
    })
}

#[op2(fast)]
fn op_dgram_close(id: u32) {
    sockets().remove(&id);
    let _ = RECV_CANCELS.cancel(id);
}

#[op2]
fn op_dgram_local_info(id: u32) -> Result<SocketInfo, JsErrorBox> {
    let socket = socket_of(id)?;
    let local = socket.local_addr().map_err(io_error)?;
    Ok(SocketInfo {
        address: local.ip().to_string(),
        family: family_name(&local.ip()),
        port: local.port(),
    })
}

#[op2]
fn op_dgram_remote_info(id: u32) -> Result<SocketInfo, JsErrorBox> {
    let socket = socket_of(id)?;
    let remote = socket.peer_addr().map_err(io_error)?;
    Ok(SocketInfo {
        address: remote.ip().to_string(),
        family: family_name(&remote.ip()),
        port: remote.port(),
    })
}

#[op2(fast)]
fn op_dgram_set_broadcast(id: u32, broadcast: bool) -> Result<(), JsErrorBox> {
    socket_of(id)?.set_broadcast(broadcast).map_err(io_error)
}

#[op2(fast)]
fn op_dgram_set_ttl(id: u32, #[smi] ttl: i32) -> Result<(), JsErrorBox> {
    let ttl = ttl_value(ttl, 1)?;
    SockRef::from(&*socket_of(id)?)
        .set_ttl(ttl)
        .map_err(io_error)
}

#[op2(fast)]
fn op_dgram_set_multicast_ttl(id: u32, #[smi] ttl: i32) -> Result<(), JsErrorBox> {
    let ttl = ttl_value(ttl, 0)?;
    let socket = socket_of(id)?;
    let sock = SockRef::from(&*socket);
    if socket.local_addr().map_err(io_error)?.is_ipv4() {
        sock.set_multicast_ttl_v4(ttl).map_err(io_error)
    } else {
        sock.set_multicast_hops_v6(ttl).map_err(io_error)
    }
}

/// libuv rejects an out-of-range TTL itself: unicast TTL starts at 1, multicast
/// TTL at 0, and both end at 255.
fn ttl_value(value: i32, minimum: i32) -> Result<u32, JsErrorBox> {
    if !(minimum..=255).contains(&value) {
        return Err(invalid());
    }
    #[expect(
        clippy::cast_sign_loss,
        reason = "the range check above proves the value is non-negative"
    )]
    Ok(value as u32)
}

#[op2(fast)]
fn op_dgram_set_multicast_loopback(id: u32, enabled: bool) -> Result<(), JsErrorBox> {
    let socket = socket_of(id)?;
    let sock = SockRef::from(&*socket);
    if socket.local_addr().map_err(io_error)?.is_ipv4() {
        sock.set_multicast_loop_v4(enabled).map_err(io_error)
    } else {
        sock.set_multicast_loop_v6(enabled).map_err(io_error)
    }
}

#[op2(fast)]
fn op_dgram_set_multicast_interface(id: u32, #[string] interface: &str) -> Result<(), JsErrorBox> {
    // libuv accepts either family regardless of the socket's own family, and parses
    // the interface before the descriptor is used, so an invalid address reports
    // EINVAL on an unbound socket just as it does on a bound one.
    if let Ok(interface) = interface.parse::<Ipv4Addr>() {
        SockRef::from(&*socket_of(id)?)
            .set_multicast_if_v4(&interface)
            .map_err(io_error)
    } else {
        let index = interface_index(interface)?;
        SockRef::from(&*socket_of(id)?)
            .set_multicast_if_v6(index)
            .map_err(io_error)
    }
}

#[op2]
fn op_dgram_add_membership(
    id: u32,
    #[string] group: &str,
    #[string] interface: Option<String>,
) -> Result<(), JsErrorBox> {
    membership(id, group, interface, true)
}

#[op2]
fn op_dgram_drop_membership(
    id: u32,
    #[string] group: &str,
    #[string] interface: Option<String>,
) -> Result<(), JsErrorBox> {
    membership(id, group, interface, false)
}

fn membership(
    id: u32,
    group: &str,
    interface: Option<String>,
    join: bool,
) -> Result<(), JsErrorBox> {
    // libuv parses the group before it inspects the handle, so an invalid address
    // reports EINVAL even on a socket that has not been bound yet.
    let group: IpAddr = group.parse().map_err(|_| invalid())?;
    let socket = socket_of(id)?;
    let sock = SockRef::from(&*socket);
    match (socket.local_addr().map_err(io_error)?.is_ipv4(), group) {
        (true, IpAddr::V4(group)) => {
            let interface = match interface {
                Some(interface) if !interface.is_empty() => {
                    interface.parse::<Ipv4Addr>().map_err(|_| invalid())?
                }
                _ => Ipv4Addr::UNSPECIFIED,
            };
            if join {
                sock.join_multicast_v4(&group, &interface)
            } else {
                sock.leave_multicast_v4(&group, &interface)
            }
            .map_err(io_error)
        }
        (false, IpAddr::V6(group)) => {
            let interface = interface.as_deref().map_or(Ok(0), interface_index)?;
            if join {
                sock.join_multicast_v6(&group, interface)
            } else {
                sock.leave_multicast_v6(&group, interface)
            }
            .map_err(io_error)
        }
        _ => Err(invalid()),
    }
}

#[op2]
fn op_dgram_add_source_membership(
    id: u32,
    #[string] source: &str,
    #[string] group: &str,
    #[string] interface: Option<String>,
) -> Result<(), JsErrorBox> {
    source_membership(id, source, group, interface, true)
}

#[op2]
fn op_dgram_drop_source_membership(
    id: u32,
    #[string] source: &str,
    #[string] group: &str,
    #[string] interface: Option<String>,
) -> Result<(), JsErrorBox> {
    source_membership(id, source, group, interface, false)
}

fn source_membership(
    id: u32,
    source: &str,
    group: &str,
    interface: Option<String>,
    join: bool,
) -> Result<(), JsErrorBox> {
    let source: Ipv4Addr = source.parse().map_err(|_| invalid())?;
    let group: Ipv4Addr = group.parse().map_err(|_| invalid())?;
    let interface = match interface {
        Some(interface) if !interface.is_empty() => {
            interface.parse::<Ipv4Addr>().map_err(|_| invalid())?
        }
        _ => Ipv4Addr::UNSPECIFIED,
    };
    let socket = socket_of(id)?;
    if !socket.local_addr().map_err(io_error)?.is_ipv4() {
        return Err(invalid());
    }
    source_membership_socket(&socket, source, group, interface, join).map_err(io_error)
}

#[cfg(unix)]
fn source_membership_socket(
    socket: &UdpSocket,
    source: Ipv4Addr,
    group: Ipv4Addr,
    interface: Ipv4Addr,
    join: bool,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let request = libc::ip_mreq_source {
        imr_multiaddr: libc::in_addr {
            s_addr: u32::from_ne_bytes(group.octets()),
        },
        imr_sourceaddr: libc::in_addr {
            s_addr: u32::from_ne_bytes(source.octets()),
        },
        imr_interface: libc::in_addr {
            s_addr: u32::from_ne_bytes(interface.octets()),
        },
    };
    let option = if join {
        libc::IP_ADD_SOURCE_MEMBERSHIP
    } else {
        libc::IP_DROP_SOURCE_MEMBERSHIP
    };
    // SAFETY: the descriptor is open and owned by `socket`, and `request` has the
    // exact layout `IP_ADD_SOURCE_MEMBERSHIP` / `IP_DROP_SOURCE_MEMBERSHIP` expect.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            option,
            (&raw const request).cast(),
            socklen_of::<libc::ip_mreq_source>(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn source_membership_socket(
    socket: &UdpSocket,
    source: Ipv4Addr,
    group: Ipv4Addr,
    interface: Ipv4Addr,
    join: bool,
) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;

    use windows_sys::Win32::Networking::WinSock::{
        IP_ADD_SOURCE_MEMBERSHIP, IP_DROP_SOURCE_MEMBERSHIP, IPPROTO_IP, setsockopt,
    };

    #[repr(C)]
    struct IpMreqSource {
        imr_multiaddr: u32,
        imr_sourceaddr: u32,
        imr_interface: u32,
    }

    let request = IpMreqSource {
        imr_multiaddr: u32::from_ne_bytes(group.octets()),
        imr_sourceaddr: u32::from_ne_bytes(source.octets()),
        imr_interface: u32::from_ne_bytes(interface.octets()),
    };
    let option = if join {
        IP_ADD_SOURCE_MEMBERSHIP
    } else {
        IP_DROP_SOURCE_MEMBERSHIP
    };
    // SAFETY: the socket is open and the request matches the Winsock option layout.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            IPPROTO_IP,
            option,
            (&raw const request).cast(),
            std::mem::size_of::<IpMreqSource>() as i32,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn source_membership_socket(
    _socket: &UdpSocket,
    _source: Ipv4Addr,
    _group: Ipv4Addr,
    _interface: Ipv4Addr,
    _join: bool,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "source-specific multicast is not supported on this platform",
    ))
}

/// Resolves the interface part of an IPv6 multicast address the way libuv's
/// `uv_ip6_addr` does: a numeric scope is used directly, and a name is looked up with
/// `if_nametoindex` on Unix (Windows scopes are numeric).
fn interface_index(interface: &str) -> Result<u32, JsErrorBox> {
    let Some((address, scope)) = interface.split_once('%') else {
        interface.parse::<Ipv6Addr>().map_err(|_| invalid())?;
        return Ok(0);
    };
    address.parse::<Ipv6Addr>().map_err(|_| invalid())?;
    if let Ok(index) = scope.parse::<u32>() {
        return Ok(index);
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;

        let name = CString::new(scope).map_err(|_| invalid())?;
        // SAFETY: `name` is a valid NUL-terminated C string that outlives the call.
        Ok(unsafe { libc::if_nametoindex(name.as_ptr()) })
    }
    #[cfg(not(unix))]
    {
        Ok(0)
    }
}

#[op2(fast)]
fn op_dgram_get_buffer_size(id: u32, recv: bool) -> Result<u32, JsErrorBox> {
    let socket = socket_of(id)?;
    let size = if recv {
        SockRef::from(&*socket).recv_buffer_size()
    } else {
        SockRef::from(&*socket).send_buffer_size()
    }
    .map_err(io_error)?;
    u32::try_from(size).map_err(|_| errno(EINVAL))
}

#[op2(fast)]
fn op_dgram_set_buffer_size(id: u32, #[smi] size: u32, recv: bool) -> Result<(), JsErrorBox> {
    let socket = socket_of(id)?;
    let size = usize::try_from(size).map_err(|_| invalid())?;
    let sock = SockRef::from(&*socket);
    if recv {
        sock.set_recv_buffer_size(size).map_err(io_error)
    } else {
        sock.set_send_buffer_size(size).map_err(io_error)
    }
}
