use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::rc::Rc;
use std::sync::Mutex;

use deno_core::{OpDecl, OpState, op2};
use deno_error::JsErrorBox;
use hickory_resolver::config::{NameServerConfig, ResolveHosts, ResolverConfig};
use hickory_resolver::lookup::Lookup;
use hickory_resolver::net::runtime::{RuntimeProvider, TokioRuntimeProvider};
use hickory_resolver::net::{DnsError, NetError};
use hickory_resolver::proto::op::ResponseCode;
use hickory_resolver::proto::rr::{RData, RecordType};
use hickory_resolver::{Resolver, ResolverBuilder, TokioResolver};
#[cfg(windows)]
use tokio::net::lookup_host;

// libuv-style getaddrinfo errno numbers (`UV__EAI_*` in libuv's errno.h), the
// error contract of `op_dns_lookup`: the JS layer reports them as
// `error.errno` and derives `error.code` the way Node's `DNSException` does
// (EAI_NODATA/EAI_NONAME become ENOTFOUND, everything else keeps its EAI_*
// name).
const EAI_AGAIN: i32 = -3001;
#[cfg(unix)]
const EAI_BADFLAGS: i32 = -3002;
const EAI_FAIL: i32 = -3004;
#[cfg(unix)]
const EAI_FAMILY: i32 = -3005;
#[cfg(unix)]
const EAI_MEMORY: i32 = -3006;
#[cfg(windows)]
const EAI_NODATA: i32 = -3007;
const EAI_NONAME: i32 = -3008;
#[cfg(unix)]
const EAI_SERVICE: i32 = -3010;
#[cfg(unix)]
const EAI_SOCKTYPE: i32 = -3011;

/// Mirrors libuv's `uv__getaddrinfo_translate_error`: the platform's EAI_*
/// codes (negative on glibc/musl, small positives on apple) become libuv's
/// stable numbers. Codes without a translation (e.g. `EAI_SYSTEM`) fall back
/// to `EAI_NONAME`, the previous behavior.
#[cfg(unix)]
const fn eai_to_uv(code: i32) -> i32 {
    match code {
        #[cfg(target_vendor = "apple")]
        2 => EAI_AGAIN,
        #[cfg(target_vendor = "apple")]
        3 => EAI_BADFLAGS,
        #[cfg(target_vendor = "apple")]
        4 => EAI_FAIL,
        #[cfg(target_vendor = "apple")]
        5 => EAI_FAMILY,
        #[cfg(target_vendor = "apple")]
        6 => EAI_MEMORY,
        #[cfg(target_vendor = "apple")]
        8 => EAI_NONAME,
        #[cfg(target_vendor = "apple")]
        9 => EAI_SERVICE,
        #[cfg(target_vendor = "apple")]
        10 => EAI_SOCKTYPE,
        // glibc and musl share this numbering for the codes below.
        #[cfg(not(target_vendor = "apple"))]
        -1 => EAI_BADFLAGS,
        #[cfg(not(target_vendor = "apple"))]
        -2 => EAI_NONAME,
        #[cfg(not(target_vendor = "apple"))]
        -3 => EAI_AGAIN,
        #[cfg(not(target_vendor = "apple"))]
        -4 => EAI_FAIL,
        #[cfg(not(target_vendor = "apple"))]
        -6 => EAI_FAMILY,
        #[cfg(not(target_vendor = "apple"))]
        -7 => EAI_SOCKTYPE,
        #[cfg(not(target_vendor = "apple"))]
        -8 => EAI_SERVICE,
        #[cfg(not(target_vendor = "apple"))]
        -10 => EAI_MEMORY,
        _ => EAI_NONAME,
    }
}

/// Windows getaddrinfo failures arrive as WSA error codes; the four below are
/// the ones libuv translates (win/getaddrinfo.c). Errors without a code (see
/// the unix note above) keep the previous ENOTFOUND result.
#[cfg(windows)]
fn wsa_to_uv(code: Option<i32>) -> i32 {
    match code {
        Some(11_001) => EAI_NONAME, // WSAHOST_NOT_FOUND
        Some(11_002) => EAI_AGAIN,  // WSATRY_AGAIN
        Some(11_003) => EAI_FAIL,   // WSANO_RECOVERY
        Some(11_004) => EAI_NODATA, // WSANO_DATA
        _ => EAI_NONAME,
    }
}

/// Calls getaddrinfo directly, the way libuv's `uv_getaddrinfo` does:
/// `std::net::ToSocketAddrs` (and with it `tokio::net::lookup_host`) drops
/// the platform's EAI_* code from its error, so the distinct codes Node
/// reports (`EAI_AGAIN`, `EAI_FAIL`, ...) are only available from the raw call.
/// Returns the addresses in getaddrinfo order, with the libuv-style errno on
/// failure.
#[cfg(unix)]
fn getaddrinfo_addresses(hostname: &str, family: u8) -> Result<Vec<IpAddr>, i32> {
    use std::ffi::CString;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::ptr;

    // A hostname cannot contain a NUL byte; report it as unresolvable.
    let Ok(name) = CString::new(hostname) else {
        return Err(EAI_NONAME);
    };
    // SAFETY: `libc::addrinfo` is a plain C struct of pointers and integers,
    // so an all-zero value is a valid (empty) hints record that is fully
    // assigned before use.
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_socktype = libc::SOCK_STREAM;
    if family == 4 {
        hints.ai_family = libc::AF_INET;
    } else if family == 6 {
        hints.ai_family = libc::AF_INET6;
    }
    let mut head: *mut libc::addrinfo = ptr::null_mut();
    // SAFETY: `hints` is fully initialized and outlives the call, and `head`
    // is a valid out-parameter. On success getaddrinfo stores a heap-allocated
    // linked list in `head` that is freed exactly once by `freeaddrinfo`
    // below.
    let status =
        unsafe { libc::getaddrinfo(name.as_ptr(), ptr::null(), &raw const hints, &raw mut head) };
    if status != 0 {
        return Err(eai_to_uv(status));
    }

    let mut addresses = Vec::new();
    let mut entry = head;
    while !entry.is_null() {
        // SAFETY: `entry` walks the list getaddrinfo returned, whose nodes
        // and `ai_addr` sockaddrs stay valid until `freeaddrinfo` below; each
        // sockaddr is read through the type its `ai_family` selects.
        let info = unsafe { &*entry };
        if info.ai_family == libc::AF_INET {
            // SAFETY: covered by the comment on the list walk above.
            let address = unsafe { &*info.ai_addr.cast::<libc::sockaddr_in>() };
            addresses.push(IpAddr::V4(Ipv4Addr::from(
                address.sin_addr.s_addr.to_ne_bytes(),
            )));
        } else if info.ai_family == libc::AF_INET6 {
            // SAFETY: covered by the comment on the list walk above.
            let address = unsafe { &*info.ai_addr.cast::<libc::sockaddr_in6>() };
            addresses.push(IpAddr::V6(Ipv6Addr::from(address.sin6_addr.s6_addr)));
        }
        entry = info.ai_next;
    }
    // SAFETY: `head` is the list getaddrinfo produced; it is released exactly
    // once here and not used afterwards.
    unsafe { libc::freeaddrinfo(head) };
    Ok(addresses)
}

/// `spawn_blocking` only fails when the task panics; `JoinError` has no JS
/// representation, so surface it as an internal failure.
#[cfg(unix)]
fn task_panic(error: &tokio::task::JoinError) -> JsErrorBox {
    JsErrorBox::new("Error", format!("ERR_DNS_INTERNAL\0{error}"))
}

pub(crate) const OPS: &[OpDecl] = &[op_dns_lookup(), op_dns_resolve(), op_dns_reverse()];

#[op2(async(lazy), fast)]
#[string]
async fn op_dns_lookup(#[string] hostname: String, family: u8) -> Result<String, JsErrorBox> {
    // getaddrinfo can block on network I/O for seconds, so it runs off the
    // event loop on both platforms, like libuv does for `uv_getaddrinfo`.
    #[cfg(unix)]
    let addresses: Vec<IpAddr> = {
        tokio::task::spawn_blocking(move || getaddrinfo_addresses(&hostname, family))
            .await
            .map_err(|error| task_panic(&error))?
            .map_err(|errno| JsErrorBox::new("Error", errno.to_string()))?
    };
    #[cfg(windows)]
    let addresses: Vec<IpAddr> = lookup_host((hostname.as_str(), 0))
        .await
        .map_err(|error: std::io::Error| {
            JsErrorBox::new("Error", wsa_to_uv(error.raw_os_error()).to_string())
        })?
        .map(|address| address.ip())
        .collect();

    let mut entries = Vec::new();
    for address in addresses {
        let (address_family, ip) = match address {
            IpAddr::V4(ip) => (4u8, ip.to_string()),
            IpAddr::V6(ip) => (6u8, ip.to_string()),
        };
        if family != 0 && family != address_family {
            continue;
        }
        entries.push(address_family.to_string());
        entries.push(ip);
    }
    if entries.is_empty() {
        return Err(JsErrorBox::new("Error", EAI_NONAME.to_string()));
    }
    Ok(entries.join("\0"))
}

/// Resolvers are `Clone` over a name-server pool and a cache, so one per server set
/// is shared by every query. Building one reads the system configuration (and
/// `/etc/hosts`), which must not happen per call.
#[derive(Debug, Default)]
pub(crate) struct Resolvers(Mutex<HashMap<String, TokioResolver>>);

impl Resolvers {
    fn get(&self, servers: &str) -> Result<TokioResolver, JsErrorBox> {
        let mut resolvers = self.0.lock().expect("dns resolver cache");
        if let Some(resolver) = resolvers.get(servers) {
            return Ok(resolver.clone());
        }
        let resolver = if servers.is_empty() {
            system_resolver()?
        } else {
            custom_resolver(servers)?
        };
        resolvers.insert(servers.to_owned(), resolver.clone());
        Ok(resolver)
    }
}

/// c-ares never reads the hosts file and never caches; keep the same contract so
/// `dns.resolve*` and `dns.lookup` do not disagree about a name.
fn configure<P: RuntimeProvider>(builder: &mut ResolverBuilder<P>) {
    builder.options_mut().cache_size = 0;
    builder.options_mut().use_hosts_file = ResolveHosts::Never;
}

fn system_resolver() -> Result<TokioResolver, JsErrorBox> {
    let mut builder = TokioResolver::builder_tokio().map_err(resolver_error)?;
    configure(&mut builder);
    builder.build().map_err(resolver_error)
}

fn custom_config(servers: &str) -> Result<ResolverConfig, JsErrorBox> {
    let mut configs = Vec::new();
    for server in servers.split(',').filter(|value| !value.is_empty()) {
        let address: SocketAddr = server
            .parse()
            .or_else(|_| {
                use std::net::ToSocketAddrs;
                (server, 53)
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut addresses| addresses.next())
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("unknown resolver address {server}"),
                        )
                    })
            })
            .map_err(|error| JsErrorBox::new("Error", format!("EINVAL\0{error}")))?;
        // c-ares retries a truncated answer over TCP; configure both transports.
        let mut name_server = NameServerConfig::udp_and_tcp(address.ip());
        for connection in &mut name_server.connections {
            connection.port = address.port();
        }
        configs.push(name_server);
    }
    Ok(ResolverConfig::from_parts(None, Vec::new(), configs))
}

fn custom_resolver(servers: &str) -> Result<TokioResolver, JsErrorBox> {
    let mut builder =
        Resolver::builder_with_config(custom_config(servers)?, TokioRuntimeProvider::default());
    configure(&mut builder);
    builder.build().map_err(resolver_error)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a direct `map_err` handler, which requires an owned error"
)]
fn resolver_error(error: NetError) -> JsErrorBox {
    JsErrorBox::new("Error", format!("ESERVFAIL\0{error}"))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a direct `map_err` handler, which requires an owned error"
)]
fn request_error(error: NetError) -> JsErrorBox {
    let code = match &error {
        NetError::Dns(DnsError::NoRecordsFound(no_records)) => {
            if no_records.response_code == ResponseCode::NXDomain {
                "ENOTFOUND"
            } else {
                "ENODATA"
            }
        }
        _ => "ESERVFAIL",
    };
    JsErrorBox::new("Error", format!("{code}\0{error}"))
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            value if (value as u32) < 0x20 => {
                write!(out, "\\u{:04x}", value as u32).expect("writing to a `String` cannot fail");
            }
            value => out.push(value),
        }
    }
    out.push('"');
    out
}

fn name_text(name: &hickory_resolver::proto::rr::Name) -> String {
    name.to_string().trim_end_matches('.').to_owned()
}

fn record_json(record: &RData) -> Option<String> {
    match record {
        RData::A(address) => Some(json_string(&address.0.to_string())),
        RData::AAAA(address) => Some(json_string(&address.0.to_string())),
        RData::CNAME(name) => Some(json_string(&name_text(&name.0))),
        RData::NS(name) => Some(json_string(&name_text(&name.0))),
        RData::PTR(name) => Some(json_string(&name_text(&name.0))),
        RData::MX(mx) => Some(format!(
            "{{\"priority\":{},\"exchange\":{}}}",
            mx.preference,
            json_string(&name_text(&mx.exchange))
        )),
        RData::TXT(txt) => {
            let chunks: Vec<String> = txt
                .txt_data
                .iter()
                .map(|chunk| json_string(&String::from_utf8_lossy(chunk)))
                .collect();
            Some(format!("[{}]", chunks.join(",")))
        }
        _ => None,
    }
}

/// Renders the answer section of a lookup as the JSON array the JS layer parses.
fn records_json(lookup: &Lookup) -> String {
    let records: Vec<String> = lookup
        .answers()
        .iter()
        .filter_map(|record| record_json(&record.data))
        .collect();
    format!("[{}]", records.join(","))
}

#[op2(async(lazy), fast)]
#[string]
async fn op_dns_resolve(
    state: Rc<RefCell<OpState>>,
    #[string] servers: String,
    #[string] hostname: String,
    #[string] rrtype: String,
) -> Result<String, JsErrorBox> {
    let record_type = match rrtype.as_str() {
        "A" => RecordType::A,
        "AAAA" => RecordType::AAAA,
        "CNAME" => RecordType::CNAME,
        "MX" => RecordType::MX,
        "TXT" => RecordType::TXT,
        "NS" => RecordType::NS,
        "PTR" => RecordType::PTR,
        _ => {
            return Err(JsErrorBox::new(
                "Error",
                format!("ERR_NOT_IMPLEMENTED\0dns.resolve({rrtype})"),
            ));
        }
    };
    let resolver = {
        let state = state.borrow();
        state.borrow::<Resolvers>().get(&servers)?
    };
    let lookup = resolver
        .lookup(hostname.as_str(), record_type)
        .await
        .map_err(request_error)?;
    Ok(records_json(&lookup))
}

#[op2(async(lazy), fast)]
#[string]
async fn op_dns_reverse(
    state: Rc<RefCell<OpState>>,
    #[string] servers: String,
    #[string] address: String,
) -> Result<String, JsErrorBox> {
    let ip: IpAddr = address
        .parse()
        .map_err(|error| JsErrorBox::new("Error", format!("EINVAL\0{error}")))?;
    let resolver = {
        let state = state.borrow();
        state.borrow::<Resolvers>().get(&servers)?
    };
    let lookup = resolver
        .reverse_lookup(hickory_resolver::proto::rr::Name::from(ip))
        .await
        .map_err(request_error)?;
    Ok(records_json(&lookup))
}

#[cfg(test)]
mod tests {
    use super::custom_config;

    #[test]
    fn custom_servers_get_udp_and_tcp_on_the_requested_port() {
        let config = custom_config("127.0.0.1:5353,::1").expect("valid server list");
        assert_eq!(config.name_servers.len(), 2);
        for server in &config.name_servers {
            assert_eq!(server.connections.len(), 2);
        }
        assert!(
            server_ports(&config.name_servers[0])
                .iter()
                .all(|port| *port == 5353)
        );
        assert!(
            server_ports(&config.name_servers[1])
                .iter()
                .all(|port| *port == 53)
        );
    }

    fn server_ports(server: &hickory_resolver::config::NameServerConfig) -> Vec<u16> {
        server
            .connections
            .iter()
            .map(|connection| connection.port)
            .collect()
    }
    #[test]
    fn resolve_resolvers_disable_hosts_and_cache() {
        use super::{ResolveHosts, Resolver, TokioRuntimeProvider, configure, custom_config};

        let mut builder = Resolver::builder_with_config(
            custom_config("127.0.0.1:53").expect("valid server"),
            TokioRuntimeProvider::default(),
        );
        configure(&mut builder);
        let resolver = builder.build().expect("the resolver builds");
        assert_eq!(resolver.options().cache_size, 0);
        assert_eq!(resolver.options().use_hosts_file, ResolveHosts::Never);
    }
}
