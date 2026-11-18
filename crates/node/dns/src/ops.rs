use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};

use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;
use hickory_resolver::ResolveErrorKind;
use hickory_resolver::config::{NameServerConfig, NameServerConfigGroup, ResolverConfig};
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::proto::ProtoErrorKind;
use hickory_resolver::proto::op::ResponseCode;
use hickory_resolver::proto::rr::{RData, RecordType};
use hickory_resolver::proto::xfer::Protocol;
use hickory_resolver::{Resolver, TokioResolver};
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

fn system_resolver() -> Result<TokioResolver, JsErrorBox> {
    let builder = TokioResolver::builder_tokio().map_err(resolver_error)?;
    Ok(builder.build())
}

fn custom_resolver(servers: &str) -> Result<TokioResolver, JsErrorBox> {
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
        configs.push(NameServerConfig {
            socket_addr: address,
            protocol: Protocol::Udp,
            tls_dns_name: None,
            http_endpoint: None,
            trust_negative_responses: true,
            bind_addr: None,
        });
    }
    let config = ResolverConfig::from_parts(None, Vec::new(), NameServerConfigGroup::from(configs));
    Ok(Resolver::builder_with_config(config, TokioConnectionProvider::default()).build())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a direct `map_err` handler, which requires an owned error"
)]
fn resolver_error(error: hickory_resolver::ResolveError) -> JsErrorBox {
    JsErrorBox::new("Error", format!("ESERVFAIL\0{error}"))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a direct `map_err` handler, which requires an owned error"
)]
fn request_error(error: hickory_resolver::ResolveError) -> JsErrorBox {
    let code = match error.kind() {
        ResolveErrorKind::Proto(proto) => match proto.kind() {
            ProtoErrorKind::NoRecordsFound { response_code, .. } => {
                if *response_code == ResponseCode::NXDomain {
                    "ENOTFOUND"
                } else {
                    "ENODATA"
                }
            }
            _ => "ESERVFAIL",
        },
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
            mx.preference(),
            json_string(&name_text(mx.exchange()))
        )),
        RData::TXT(txt) => {
            let chunks: Vec<String> = txt
                .txt_data()
                .iter()
                .map(|chunk| json_string(&String::from_utf8_lossy(chunk)))
                .collect();
            Some(format!("[{}]", chunks.join(",")))
        }
        _ => None,
    }
}

#[op2(async(lazy), fast)]
#[string]
async fn op_dns_resolve(
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
    let resolver = if servers.is_empty() {
        system_resolver()?
    } else {
        custom_resolver(&servers)?
    };
    let lookup = resolver
        .lookup(hostname.as_str(), record_type)
        .await
        .map_err(request_error)?;
    let records: Vec<String> = lookup.iter().filter_map(record_json).collect();
    Ok(format!("[{}]", records.join(",")))
}

#[op2(async(lazy), fast)]
#[string]
async fn op_dns_reverse(
    #[string] servers: String,
    #[string] address: String,
) -> Result<String, JsErrorBox> {
    let ip = address
        .parse()
        .map_err(|error| JsErrorBox::new("Error", format!("EINVAL\0{error}")))?;
    let resolver = if servers.is_empty() {
        system_resolver()?
    } else {
        custom_resolver(&servers)?
    };
    let lookup = resolver.reverse_lookup(ip).await.map_err(request_error)?;
    let names: Vec<String> = lookup
        .iter()
        .map(|name| json_string(&name_text(&name.0)))
        .collect();
    Ok(format!("[{}]", names.join(",")))
}
