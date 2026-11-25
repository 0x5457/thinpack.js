//! Typed results shared by the HTTP/2 ops and `node:http2`.
//!
//! Node bindings return structured values instead of hand-encoded byte blobs: every result
//! implements `deno_core::ToV8`, so the Rust shape and the JavaScript consumer are the same
//! declaration. Events are adjacently tagged (`{ type, value }`) so JavaScript dispatches on a
//! stable name, header entries are `[name, value]` tuples of Latin-1 strings that keep the
//! HTTP/2 field bytes without inventing an object, and payloads use zero-copy `Uint8Array`s.
//! Other built-in modules can adopt the same convention when their hand-written encodings are
//! next touched.

use deno_core::ToV8;
use deno_core::convert::{ByteString, OptionUndefined, Uint8Array};
use h2::RecvStream;
use http::{HeaderMap, Request};

/// HTTP/2 fields in wire order. Duplicates and `set-cookie` are meaningful, so this is a list
/// of pairs rather than an object.
pub(crate) type Headers = Vec<(ByteString, ByteString)>;

pub(crate) fn header_entries(headers: &HeaderMap) -> Headers {
    headers
        .iter()
        .map(|(name, value)| {
            (
                ByteString::from(name.as_str()),
                ByteString::from(value.as_bytes()),
            )
        })
        .collect()
}

pub(crate) fn request_entries(request: &Request<RecvStream>) -> Headers {
    let mut entries: Headers = vec![(
        ByteString::from(":method"),
        ByteString::from(request.method().as_str()),
    )];
    if let Some(scheme) = request.uri().scheme_str() {
        entries.push((ByteString::from(":scheme"), ByteString::from(scheme)));
    }
    if let Some(authority) = request.uri().authority() {
        entries.push((
            ByteString::from(":authority"),
            ByteString::from(authority.as_str()),
        ));
    }
    if let Some(path) = request.uri().path_and_query() {
        entries.push((ByteString::from(":path"), ByteString::from(path.as_str())));
    }
    entries.extend(header_entries(request.headers()));
    entries
}

#[derive(ToV8)]
#[to_v8(tag = "type", content = "value")]
pub(crate) enum StreamEvent {
    Response {
        end_stream: bool,
        status: u16,
        headers: Headers,
    },
    Data(Uint8Array),
    Trailers(Headers),
    End,
    Close {
        reason: u32,
    },
    #[to_v8(rename = "error")]
    Failure(String),
}

#[derive(ToV8)]
#[to_v8(tag = "type", content = "value")]
pub(crate) enum SessionEvent {
    Stream {
        id: u32,
        headers: Headers,
    },
    Goaway {
        code: u32,
        last_stream_id: u32,
        opaque: OptionUndefined<Uint8Array>,
    },
    Close,
    #[to_v8(rename = "error")]
    Failure(String),
}

#[derive(Clone, ToV8)]
pub(crate) struct SessionInfo {
    pub local: String,
    pub remote: String,
}

#[derive(ToV8)]
pub(crate) struct ListenerInfo {
    pub id: u32,
    pub local: String,
}
