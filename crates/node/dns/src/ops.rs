use std::fmt::Write as _;
use std::net::SocketAddr;

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
use tokio::net::lookup_host;

/// libuv reports "not found" as errno 2; the JS layer maps any integer to ENOTFOUND.
const EAI_NONAME: i32 = -2;

pub(crate) const OPS: &[OpDecl] = &[op_dns_lookup(), op_dns_resolve(), op_dns_reverse()];

#[op2(async(lazy), fast)]
#[string]
async fn op_dns_lookup(#[string] hostname: String, family: u8) -> Result<String, JsErrorBox> {
    let addresses =
        lookup_host((hostname.as_str(), 0))
            .await
            .map_err(|error: std::io::Error| {
                let errno = error.raw_os_error().unwrap_or(EAI_NONAME);
                JsErrorBox::new("Error", (-errno).to_string())
            })?;

    let mut entries = Vec::new();
    for address in addresses {
        let (address_family, ip) = match address.ip() {
            std::net::IpAddr::V4(ip) => (4u8, ip.to_string()),
            std::net::IpAddr::V6(ip) => (6u8, ip.to_string()),
        };
        if family != 0 && family != address_family {
            continue;
        }
        entries.push(address_family.to_string());
        entries.push(ip);
    }
    if entries.is_empty() {
        return Err(JsErrorBox::new("Error", (-EAI_NONAME).to_string()));
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
