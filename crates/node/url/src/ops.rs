use deno_core::{OpDecl, op2};
use url::{Host, Url};

pub(crate) const OPS: &[OpDecl] = &[op_url_parse(), op_url_apply(), op_url_domain_to_ascii()];

#[op2]
#[string]
fn op_url_parse(#[string] input: &str, #[string] base: &str) -> String {
    let parsed = if base.is_empty() {
        Url::parse(input)
    } else {
        match Url::parse(base) {
            Ok(base) => Url::options().base_url(Some(&base)).parse(input),
            Err(_) => return String::new(),
        }
    };
    parsed.map(|url| encode(&url)).unwrap_or_default()
}

#[op2]
#[string]
fn op_url_apply(#[string] href: &str, #[string] name: &str, #[string] value: &str) -> String {
    let Ok(mut url) = Url::parse(href) else {
        return String::new();
    };

    match name {
        "href" => match Url::parse(value) {
            Ok(replacement) => url = replacement,
            Err(_) => return String::new(),
        },
        "protocol" => {
            let _ = url.set_scheme(value.strip_suffix(':').unwrap_or(value));
        }
        "username" => {
            let _ = url.set_username(value);
        }
        "password" => {
            let password = if value.is_empty() { None } else { Some(value) };
            let _ = url.set_password(password);
        }
        "host" => set_host(&mut url, value),
        "hostname" => {
            let _ = url.set_host(Some(value));
        }
        "port" => set_port(&mut url, value),
        "pathname" => {
            if !url.cannot_be_a_base() {
                url.set_path(value);
            }
        }
        "search" => {
            if value.is_empty() {
                url.set_query(None);
            } else {
                url.set_query(Some(value.strip_prefix('?').unwrap_or(value)));
            }
        }
        "hash" => {
            if value.is_empty() {
                url.set_fragment(None);
            } else {
                url.set_fragment(Some(value.strip_prefix('#').unwrap_or(value)));
            }
        }
        _ => {}
    }

    encode(&url)
}

#[op2]
#[string]
fn op_url_domain_to_ascii(#[string] domain: &str) -> String {
    Host::parse(domain)
        .map(|host| host.to_string())
        .unwrap_or_default()
}

fn encode(url: &Url) -> String {
    let hostname = url.host_str().unwrap_or_default();
    let port = url.port().map(|port| port.to_string()).unwrap_or_default();
    let host = if port.is_empty() {
        hostname.to_owned()
    } else {
        format!("{hostname}:{port}")
    };
    let protocol = format!("{}:", url.scheme());
    let search = match url.query() {
        Some(query) if !query.is_empty() => format!("?{query}"),
        _ => String::new(),
    };
    let hash = match url.fragment() {
        Some(fragment) if !fragment.is_empty() => format!("#{fragment}"),
        _ => String::new(),
    };
    let origin = origin(url);

    [
        url.as_str(),
        &protocol,
        url.username(),
        url.password().unwrap_or_default(),
        &host,
        hostname,
        &port,
        url.path(),
        &search,
        &hash,
        &origin,
    ]
    .join("\0")
}

fn origin(url: &Url) -> String {
    if !matches!(url.scheme(), "http" | "https" | "ws" | "wss" | "ftp") {
        return "null".to_owned();
    }
    let hostname = url.host_str().unwrap_or_default();
    url.port().map_or_else(
        || format!("{}://{hostname}", url.scheme()),
        |port| format!("{}://{hostname}:{port}", url.scheme()),
    )
}

fn set_host(url: &mut Url, value: &str) {
    let (host, port) = split_host_port(value);
    if url.set_host(Some(host)).is_err() {
        return;
    }
    if let Some(port) = port {
        set_port(url, port);
    }
}

fn split_host_port(value: &str) -> (&str, Option<&str>) {
    let host_end = value.strip_prefix('[').map_or_else(
        || value.find(':').unwrap_or(value.len()),
        |rest| rest.find(']').map_or(value.len(), |end| end + 2),
    );
    (&value[..host_end], value[host_end..].strip_prefix(':'))
}

fn set_port(url: &mut Url, value: &str) {
    if value.is_empty() {
        let _ = url.set_port(None);
        return;
    }
    let end = value.bytes().take_while(u8::is_ascii_digit).count();
    if let Ok(port) = value[..end].parse::<u16>() {
        let _ = url.set_port(Some(port));
    }
}
