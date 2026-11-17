use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;
use h2::client::{self, SendRequest};
use h2::server::{Connection as ServerConnection, SendResponse};
use h2::{PingPong, Reason, RecvStream, SendStream};
use http::{HeaderMap, Request, Response, Version};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

pub(crate) const OPS: &[OpDecl] = &[
    op_http2_connect(),
    op_http2_connect_tls(),
    op_http2_request(),
    op_http2_write(),
    op_http2_next_event(),
    op_http2_reset_stream(),
    op_http2_close_session(),
    op_http2_session_info(),
    op_http2_goaway(),
    op_http2_listen(),
    op_http2_listen_secure(),
    op_http2_set_secure_context(),
    op_http2_listener_close(),
    op_http2_listener_info(),
    op_http2_accept_session(),
    op_http2_session_event(),
    op_http2_respond(),
    op_http2_send_trailers(),
    op_http2_ping(),
];

const EVENT_RESPONSE: u8 = 1;
const EVENT_DATA: u8 = 2;
const EVENT_TRAILERS: u8 = 3;
const EVENT_END: u8 = 4;
const EVENT_CLOSE: u8 = 5;
const EVENT_ERROR: u8 = 6;
const EVENT_STREAM: u8 = 7;
const EVENT_GOAWAY: u8 = 8;

enum SessionCommand {
    GoAway(Reason),
}

enum Inject {
    Frame(Vec<u8>),
    Close,
}

struct Session {
    send: Mutex<Option<SendRequest<Bytes>>>,
    streams: Mutex<HashMap<u32, Arc<Stream>>>,
    driver: Mutex<Option<tokio::task::JoinHandle<()>>>,
    events: mpsc::UnboundedSender<Vec<u8>>,
    incoming: tokio::sync::Mutex<mpsc::UnboundedReceiver<Vec<u8>>>,
    info: String,
    ping: tokio::sync::Mutex<Option<PingPong>>,
    control: mpsc::UnboundedSender<SessionCommand>,
    inject: Mutex<Option<mpsc::UnboundedSender<Inject>>>,
}

struct Stream {
    session: u32,
    events: mpsc::UnboundedSender<Vec<u8>>,
    incoming: tokio::sync::Mutex<mpsc::UnboundedReceiver<Vec<u8>>>,
    outbound: tokio::sync::Mutex<Option<Outbound>>,
}

enum Outbound {
    Pending(SendResponse<Bytes>),
    Open(SendStream<Bytes>),
}

fn sessions() -> &'static Mutex<HashMap<u32, Arc<Session>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<u32, Arc<Session>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn streams() -> &'static Mutex<HashMap<u32, Arc<Stream>>> {
    static STREAMS: OnceLock<Mutex<HashMap<u32, Arc<Stream>>>> = OnceLock::new();
    STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

static SESSIONS: thinpack_core::IdCounter = thinpack_core::IdCounter::new();
static STREAMS: thinpack_core::IdCounter = thinpack_core::IdCounter::new();
static LISTENERS: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

fn next_session_id() -> u32 {
    SESSIONS.next()
}

fn next_stream_id() -> u32 {
    STREAMS.next()
}

fn session_of(id: u32) -> Result<Arc<Session>, JsErrorBox> {
    sessions()
        .lock()
        .expect("http2 session table")
        .get(&id)
        .cloned()
        .ok_or_else(|| JsErrorBox::new("Error", "ERR_HTTP2_INVALID_SESSION\0session is closed"))
}

fn stream_of(id: u32) -> Result<Arc<Stream>, JsErrorBox> {
    streams()
        .lock()
        .expect("http2 stream table")
        .get(&id)
        .cloned()
        .ok_or_else(|| JsErrorBox::new("Error", "ERR_HTTP2_INVALID_STREAM\0stream is closed"))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a direct `map_err` handler, which requires an owned error"
)]
fn http2_error(error: h2::Error) -> JsErrorBox {
    JsErrorBox::new("Error", format!("ERR_HTTP2_ERROR\0{error}"))
}

fn invalid(detail: &str) -> JsErrorBox {
    JsErrorBox::new("Error", format!("ERR_HTTP2_ERROR\0{detail}"))
}

fn closed_error() -> JsErrorBox {
    JsErrorBox::new("Error", "ERR_HTTP2_STREAM_ERROR\0stream is closed")
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a direct `map_err` handler, which requires an owned error"
)]
fn io_error(error: io::Error) -> JsErrorBox {
    let errno = error.raw_os_error().map_or(5, |code| -code);
    JsErrorBox::new("Error", errno.to_string())
}

fn insert_session(session: Session) -> u32 {
    let id = next_session_id();
    sessions()
        .lock()
        .expect("http2 session table")
        .insert(id, Arc::new(session));
    id
}

fn new_session(
    send: Option<SendRequest<Bytes>>,
    info: String,
    ping: Option<PingPong>,
    inject: Option<mpsc::UnboundedSender<Inject>>,
) -> (Session, mpsc::UnboundedReceiver<SessionCommand>) {
    let (events, incoming) = mpsc::unbounded_channel();
    let (control, commands) = mpsc::unbounded_channel();
    (
        Session {
            send: Mutex::new(send),
            streams: Mutex::new(HashMap::new()),
            driver: Mutex::new(None),
            events,
            incoming: tokio::sync::Mutex::new(incoming),
            info,
            ping: tokio::sync::Mutex::new(ping),
            control,
            inject: Mutex::new(inject),
        },
        commands,
    )
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the header blob is a fixed-width wire format: the count and name lengths are read back as u16 and value lengths as u32; HTTP/2 caps header blocks at 16 KiB"
)]
fn push_entries(event: &mut Vec<u8>, entries: &[(Vec<u8>, Vec<u8>)]) {
    event.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (name, value) in entries {
        event.extend_from_slice(&(name.len() as u16).to_le_bytes());
        event.extend_from_slice(name);
        event.extend_from_slice(&(value.len() as u32).to_le_bytes());
        event.extend_from_slice(value);
    }
}

fn push_headers(event: &mut Vec<u8>, headers: &HeaderMap) {
    let entries: Vec<(Vec<u8>, Vec<u8>)> = headers
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect();
    push_entries(event, &entries);
}

fn push_request_headers(event: &mut Vec<u8>, request: &Request<RecvStream>) {
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = vec![(
        b":method".to_vec(),
        request.method().as_str().as_bytes().to_vec(),
    )];
    if let Some(scheme) = request.uri().scheme_str() {
        entries.push((b":scheme".to_vec(), scheme.as_bytes().to_vec()));
    }
    if let Some(authority) = request.uri().authority() {
        entries.push((
            b":authority".to_vec(),
            authority.as_str().as_bytes().to_vec(),
        ));
    }
    if let Some(path) = request.uri().path_and_query() {
        entries.push((b":path".to_vec(), path.as_str().as_bytes().to_vec()));
    }
    for (name, value) in request.headers() {
        entries.push((name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()));
    }
    push_entries(event, &entries);
}

fn request_from_headers(blob: &[u8]) -> Result<Request<()>, JsErrorBox> {
    let (count, mut offset) = read_u16(blob, 0).ok_or_else(|| invalid("truncated header block"))?;
    let mut method = "GET".to_owned();
    let mut scheme = "http".to_owned();
    let mut authority = String::new();
    let mut path = "/".to_owned();
    let mut headers = Vec::new();
    for _ in 0..count {
        let (name, value, next) = read_entry(blob, offset)?;
        offset = next;
        match name.as_str() {
            ":method" => method = value,
            ":scheme" => scheme = value,
            ":authority" => authority = value,
            ":path" => path = value,
            _ => headers.push((name, value)),
        }
    }
    let uri = if authority.is_empty() {
        format!("{scheme}://{path}")
    } else {
        format!("{scheme}://{authority}{path}")
    };
    let mut builder = Request::builder()
        .method(method.as_str())
        .version(Version::HTTP_2)
        .uri(uri);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
        .body(())
        .map_err(|error| invalid(&error.to_string()))
}

fn response_from_headers(blob: &[u8]) -> Result<Response<()>, JsErrorBox> {
    let (count, mut offset) = read_u16(blob, 0).ok_or_else(|| invalid("truncated header block"))?;
    let mut status = 200u16;
    let mut headers = Vec::new();
    for _ in 0..count {
        let (name, value, next) = read_entry(blob, offset)?;
        offset = next;
        if name == ":status" {
            status = value.parse().map_err(|_| invalid("invalid :status"))?;
        } else if name.starts_with(':') {
            return Err(invalid(&format!("unsupported pseudo header {name}")));
        } else {
            headers.push((name, value));
        }
    }
    let mut builder = Response::builder().version(Version::HTTP_2).status(status);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
        .body(())
        .map_err(|error| invalid(&error.to_string()))
}

fn read_entry(blob: &[u8], offset: usize) -> Result<(String, String, usize), JsErrorBox> {
    let (name_len, offset) =
        read_u16(blob, offset).ok_or_else(|| invalid("truncated header name"))?;
    let name = string_of(blob, offset, name_len as usize)?;
    let offset = offset + name_len as usize;
    let (value_len, offset) =
        read_u32(blob, offset).ok_or_else(|| invalid("truncated header value"))?;
    let value = string_of(blob, offset, value_len as usize)?;
    Ok((name, value, offset + value_len as usize))
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<(u16, usize)> {
    let slice = bytes.get(offset..offset + 2)?;
    Some((u16::from_le_bytes([slice[0], slice[1]]), offset + 2))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<(u32, usize)> {
    let slice = bytes.get(offset..offset + 4)?;
    Some((
        u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]),
        offset + 4,
    ))
}

fn string_of(bytes: &[u8], offset: usize, len: usize) -> Result<String, JsErrorBox> {
    let slice = bytes
        .get(offset..offset + len)
        .ok_or_else(|| invalid("truncated header block"))?;
    String::from_utf8(slice.to_vec()).map_err(|_| invalid("header is not ASCII"))
}

fn trailers_from_headers(blob: &[u8]) -> Result<HeaderMap, JsErrorBox> {
    let (count, mut offset) = read_u16(blob, 0).ok_or_else(|| invalid("truncated header block"))?;
    let mut trailers = HeaderMap::new();
    for _ in 0..count {
        let (name, value, next) = read_entry(blob, offset)?;
        offset = next;
        let name = http::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| invalid(&format!("invalid header name {name}")))?;
        let value = http::header::HeaderValue::from_str(&value)
            .map_err(|_| invalid(&format!("invalid header value for {name}")))?;
        trailers.insert(name, value);
    }
    Ok(trailers)
}

#[op2]
async fn op_http2_connect(tcp_id: u32) -> Result<u32, JsErrorBox> {
    let tcp = thinpack_node_net::take_stream(tcp_id).map_err(|error| match error {
        thinpack_node_net::TakeStreamError::NotFound => {
            JsErrorBox::new("Error", "EBADF\0unknown connection")
        }
        thinpack_node_net::TakeStreamError::Busy => {
            JsErrorBox::new("Error", "EBUSY\0connection is busy")
        }
        thinpack_node_net::TakeStreamError::NotTcp => JsErrorBox::new(
            "Error",
            "ERR_NOT_IMPLEMENTED\0HTTP/2 over a Unix domain socket",
        ),
        thinpack_node_net::TakeStreamError::Io(error) => JsErrorBox::new(
            "Error",
            format!("-{}\0{error}", error.raw_os_error().unwrap_or(5)),
        ),
    })?;
    connect_session(tcp).await
}

fn injectable_transport<S>(
    transport: S,
    observer: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
) -> (tokio::io::DuplexStream, mpsc::UnboundedSender<Inject>)
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (h2_io, proxy_io) = tokio::io::duplex(64 * 1024);
    let (inject, mut frames): (
        mpsc::UnboundedSender<Inject>,
        mpsc::UnboundedReceiver<Inject>,
    ) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (mut transport_read, mut transport_write) = tokio::io::split(transport);
        let (mut h2_read, mut h2_write) = tokio::io::split(proxy_io);
        let (closed, mut remote_closed) = tokio::sync::oneshot::channel();
        let reader_observer = Arc::clone(&observer);
        let reader = tokio::spawn(async move {
            let mut buffer = vec![0u8; 16 * 1024];
            let mut pending = Vec::new();
            loop {
                match transport_read.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        observe_frames(&mut pending, &buffer[..count], &reader_observer);
                        if h2_write.write_all(&buffer[..count]).await.is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = closed.send(());
        });
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            tokio::select! {
                _ = &mut remote_closed => break,
                read = h2_read.read(&mut buffer) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if transport_write.write_all(&buffer[..count]).await.is_err() {
                            break;
                        }
                    }
                },
                frame = frames.recv() => match frame {
                    Some(Inject::Frame(frame)) => {
                        if transport_write.write_all(&frame).await.is_err() {
                            break;
                        }
                    }
                    Some(Inject::Close) | None => {
                        let _ = transport_write.shutdown().await;
                        break;
                    }
                },
            }
        }
        reader.abort();
    });
    (h2_io, inject)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "RFC 9113 encodes the frame payload length in a 24-bit field, so the low three bytes of the length are exactly the frame header"
)]
fn goaway_frame(last_stream_id: u32, code: u32, debug: &[u8]) -> Vec<u8> {
    let length = 8 + debug.len();
    let mut frame = Vec::with_capacity(9 + length);
    frame.extend_from_slice(&[
        (length >> 16) as u8,
        (length >> 8) as u8,
        length as u8,
        0x07,
        0,
        0,
        0,
        0,
        0,
    ]);
    frame.extend_from_slice(&(last_stream_id & 0x7fff_ffff).to_be_bytes());
    frame.extend_from_slice(&code.to_be_bytes());
    frame.extend_from_slice(debug);
    frame
}

fn observe_frames(
    pending: &mut Vec<u8>,
    data: &[u8],
    observer: &Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
) {
    pending.extend_from_slice(data);
    loop {
        if pending.len() < 9 {
            return;
        }
        let length =
            ((pending[0] as usize) << 16) | ((pending[1] as usize) << 8) | pending[2] as usize;
        if pending.len() < 9 + length {
            return;
        }
        if pending[3] == 0x07 && length >= 8 {
            let code = u32::from_be_bytes([pending[13], pending[14], pending[15], pending[16]]);
            let last = u32::from_be_bytes([pending[9], pending[10], pending[11], pending[12]])
                & 0x7fff_ffff;
            let debug = &pending[17..9 + length];
            let mut event = Vec::with_capacity(9 + debug.len());
            event.push(EVENT_GOAWAY);
            event.extend_from_slice(&code.to_le_bytes());
            event.extend_from_slice(&last.to_le_bytes());
            event.extend_from_slice(debug);
            if let Some(sender) = observer.lock().expect("http2 goaway observer").as_ref() {
                let _ = sender.send(event);
            }
        }
        pending.drain(..9 + length);
    }
}

#[op2]
async fn op_http2_connect_tls(tls_id: u32) -> Result<u32, JsErrorBox> {
    let tls = thinpack_node_tls::take_connection(tls_id).map_err(|error| match error {
        thinpack_node_tls::TakeError::NotFound => {
            JsErrorBox::new("Error", "EBADF\0unknown connection")
        }
        thinpack_node_tls::TakeError::Busy => JsErrorBox::new("Error", "EBUSY\0connection is busy"),
    })?;
    connect_session(tls).await
}

async fn connect_session<S>(transport: S) -> Result<u32, JsErrorBox>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let observer = Arc::new(Mutex::new(None));
    let (transport, inject) = injectable_transport(transport, Arc::clone(&observer));
    let (send, mut connection) = client::handshake(transport).await.map_err(http2_error)?;
    let send = send.ready().await.map_err(http2_error)?;
    let ping = connection.ping_pong();
    let (session, _commands) = new_session(Some(send), String::new(), ping, Some(inject));
    let id = insert_session(session);
    let events = session_of(id)?.events.clone();
    *observer.lock().expect("http2 goaway observer") = Some(events.clone());
    let driver = tokio::spawn(async move {
        let _ = connection.await;
        let _ = events.send(Vec::from([EVENT_CLOSE]));
        let _ = events.send(Vec::new());
    });
    *session_of(id)?.driver.lock().expect("http2 driver") = Some(driver);
    Ok(id)
}

#[op2(fast)]
fn op_http2_request(
    session_id: u32,
    #[buffer] headers: &[u8],
    end_stream: bool,
) -> Result<u32, JsErrorBox> {
    let session = session_of(session_id)?;
    let request = request_from_headers(headers)?;
    let stream_id = next_stream_id();
    let mut send = session.send.lock().expect("http2 send");
    let send = send
        .as_mut()
        .ok_or_else(|| JsErrorBox::new("Error", "ERR_HTTP2_INVALID_SESSION\0session is closed"))?;
    let (response, send_stream) = send
        .send_request(request, end_stream)
        .map_err(http2_error)?;
    let (events, incoming) = mpsc::unbounded_channel();
    let stream = Arc::new(Stream {
        session: session_id,
        events: events.clone(),
        incoming: tokio::sync::Mutex::new(incoming),
        outbound: tokio::sync::Mutex::new(Some(Outbound::Open(send_stream))),
    });
    session
        .streams
        .lock()
        .expect("http2 session streams")
        .insert(stream_id, stream.clone());
    streams()
        .lock()
        .expect("http2 stream table")
        .insert(stream_id, stream);
    tokio::spawn(read_response(response, events));
    Ok(stream_id)
}

async fn read_response(response: client::ResponseFuture, events: mpsc::UnboundedSender<Vec<u8>>) {
    let response = match response.await {
        Ok(response) => response,
        Err(error) => {
            send_event(&events, Vec::from([EVENT_ERROR]), &error.to_string());
            return;
        }
    };
    let (parts, body) = response.into_parts();
    let mut event = vec![EVENT_RESPONSE, u8::from(body.is_end_stream())];
    event.extend_from_slice(&parts.status.as_u16().to_le_bytes());
    push_headers(&mut event, &parts.headers);
    if events.send(event).is_err() {
        return;
    }
    read_body(body, events).await;
}

async fn read_body(mut body: RecvStream, events: mpsc::UnboundedSender<Vec<u8>>) {
    if !body.is_end_stream() {
        loop {
            match body.data().await {
                Some(Ok(chunk)) => {
                    let mut event = Vec::with_capacity(chunk.len() + 1);
                    event.push(EVENT_DATA);
                    event.extend_from_slice(&chunk);
                    if events.send(event).is_err() {
                        return;
                    }
                }
                Some(Err(error)) => {
                    if let Some(reason) = error.reason() {
                        let mut event = Vec::from([EVENT_CLOSE]);
                        event.extend_from_slice(&u32::from(reason).to_le_bytes());
                        let _ = events.send(event);
                        let _ = events.send(Vec::new());
                        return;
                    }
                    send_event(&events, Vec::from([EVENT_ERROR]), &error.to_string());
                    return;
                }
                None => break,
            }
        }
        match body.trailers().await {
            Ok(Some(trailers)) => {
                let mut event = vec![EVENT_TRAILERS];
                push_headers(&mut event, &trailers);
                if events.send(event).is_err() {
                    return;
                }
            }
            Ok(None) => {}
            Err(error) => {
                send_event(&events, Vec::from([EVENT_ERROR]), &error.to_string());
                return;
            }
        }
    }
    let _ = events.send(Vec::from([EVENT_END]));
    let mut close = Vec::from([EVENT_CLOSE]);
    close.extend_from_slice(&0u32.to_le_bytes());
    if events.send(close).is_ok() {
        let _ = events.send(Vec::new());
    }
}

fn send_event(events: &mpsc::UnboundedSender<Vec<u8>>, mut event: Vec<u8>, message: &str) {
    event.extend_from_slice(message.as_bytes());
    let _ = events.send(event);
}

#[op2]
async fn op_http2_write(
    stream_id: u32,
    #[buffer(copy)] data: Vec<u8>,
    end_stream: bool,
) -> Result<(), JsErrorBox> {
    let stream = stream_of(stream_id)?;
    let mut outbound = stream.outbound.lock().await;
    {
        let sender = match outbound.as_mut() {
            Some(Outbound::Open(sender)) => sender,
            Some(Outbound::Pending(_)) => {
                return Err(invalid("respond() must be called before writing"));
            }
            None => return Err(closed_error()),
        };
        let mut data = Bytes::from(data);
        if data.is_empty() {
            if end_stream {
                sender.send_data(Bytes::new(), true).map_err(http2_error)?;
            }
        } else {
            while !data.is_empty() {
                sender.reserve_capacity(data.len());
                let available = std::future::poll_fn(|context| sender.poll_capacity(context))
                    .await
                    .ok_or_else(closed_error)?
                    .map_err(http2_error)?;
                let chunk = data.split_to(available.min(data.len()));
                sender
                    .send_data(chunk, end_stream && data.is_empty())
                    .map_err(http2_error)?;
            }
        }
    }
    if end_stream {
        *outbound = None;
    }
    Ok(())
}

#[op2]
async fn op_http2_respond(
    stream_id: u32,
    #[buffer(copy)] headers: Vec<u8>,
    end_stream: bool,
) -> Result<(), JsErrorBox> {
    let stream = stream_of(stream_id)?;
    let response = response_from_headers(&headers)?;
    let mut outbound = stream.outbound.lock().await;
    let mut pending = match outbound.take() {
        Some(Outbound::Pending(response_handle)) => response_handle,
        Some(Outbound::Open(sender)) => {
            *outbound = Some(Outbound::Open(sender));
            return Err(invalid("respond() has already been called"));
        }
        None => return Err(closed_error()),
    };
    let sender = pending
        .send_response(response, end_stream)
        .map_err(http2_error)?;
    if !end_stream {
        *outbound = Some(Outbound::Open(sender));
    }
    Ok(())
}

#[op2]
async fn op_http2_send_trailers(
    stream_id: u32,
    #[buffer(copy)] headers: Vec<u8>,
) -> Result<(), JsErrorBox> {
    let stream = stream_of(stream_id)?;
    let trailers = trailers_from_headers(&headers)?;
    let mut outbound = stream.outbound.lock().await;
    match outbound.take() {
        Some(Outbound::Open(mut sender)) => sender.send_trailers(trailers).map_err(http2_error),
        Some(Outbound::Pending(response)) => {
            *outbound = Some(Outbound::Pending(response));
            Err(invalid("respond() must be called before sending trailers"))
        }
        None => Err(closed_error()),
    }
}

#[op2(async(lazy), fast)]
#[buffer]
async fn op_http2_next_event(stream_id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let stream = stream_of(stream_id)?;
    let mut incoming = stream.incoming.lock().await;
    Ok(match incoming.recv().await {
        Some(event) => event.into_boxed_slice(),
        None => Box::new([]),
    })
}

#[op2]
async fn op_http2_reset_stream(stream_id: u32, reason: u32) {
    let Some(stream) = streams()
        .lock()
        .expect("http2 stream table")
        .remove(&stream_id)
    else {
        return;
    };
    let session = sessions()
        .lock()
        .expect("http2 session table")
        .get(&stream.session)
        .cloned();
    if let Some(session) = session {
        session
            .streams
            .lock()
            .expect("http2 session streams")
            .remove(&stream_id);
    }
    let mut outbound = stream.outbound.lock().await;
    match outbound.take() {
        Some(Outbound::Open(mut sender)) => sender.send_reset(Reason::from(reason)),
        Some(Outbound::Pending(mut response)) => response.send_reset(Reason::from(reason)),
        None => {}
    }
}

#[op2(fast)]
fn op_http2_close_session(session_id: u32) {
    let Some(session) = sessions()
        .lock()
        .expect("http2 session table")
        .remove(&session_id)
    else {
        return;
    };
    session.send.lock().expect("http2 send").take();
    let driver = session.driver.lock().expect("http2 driver").take();
    if let Some(driver) = driver {
        driver.abort();
    }
    let inject = session.inject.lock().expect("http2 inject").take();
    if let Some(inject) = inject {
        let _ = inject.send(Inject::Close);
    }
    let pending = std::mem::take(&mut *session.streams.lock().expect("http2 session streams"));
    let mut table = streams().lock().expect("http2 stream table");
    for (stream_id, stream) in pending {
        let _ = stream.events.send(Vec::new());
        table.remove(&stream_id);
    }
    let _ = session.events.send(Vec::new());
}

#[op2]
#[string]
fn op_http2_session_info(session_id: u32) -> String {
    sessions()
        .lock()
        .expect("http2 session table")
        .get(&session_id)
        .map(|session| session.info.clone())
        .unwrap_or_default()
}

#[op2(fast)]
fn op_http2_goaway(
    session_id: u32,
    code: u32,
    last_stream_id: u32,
    #[buffer] opaque: &[u8],
) -> Result<(), JsErrorBox> {
    let session = session_of(session_id)?;
    let is_client = session.send.lock().expect("http2 send").is_some();
    if is_client || last_stream_id != 0 || !opaque.is_empty() {
        let inject = session
            .inject
            .lock()
            .expect("http2 inject")
            .clone()
            .ok_or_else(|| {
                JsErrorBox::new("Error", "ERR_HTTP2_INVALID_SESSION\0session is closed")
            })?;
        inject
            .send(Inject::Frame(goaway_frame(last_stream_id, code, opaque)))
            .map_err(|_| {
                JsErrorBox::new("Error", "ERR_HTTP2_INVALID_SESSION\0session is closed")
            })?;
        return Ok(());
    }
    session
        .control
        .send(SessionCommand::GoAway(Reason::from(code)))
        .map_err(|_| JsErrorBox::new("Error", "ERR_HTTP2_INVALID_SESSION\0session is closed"))
}

struct Listener {
    tcp: TcpListener,
    acceptor: Mutex<Option<Arc<thinpack_node_tls::ServerAcceptor>>>,
    local: String,
    notify: Arc<tokio::sync::Notify>,
}

fn listeners() -> &'static Mutex<HashMap<u32, Arc<Listener>>> {
    static LISTENERS: OnceLock<Mutex<HashMap<u32, Arc<Listener>>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_listener_id() -> u32 {
    LISTENERS.next()
}

const ALPN_H2: &str = "h2";
const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

#[rustfmt::skip]
const BAD_PREFACE_REPLY: [u8; 26] = [
    0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x08, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
];

fn register_listener(
    listener: TcpListener,
    acceptor: Option<thinpack_node_tls::ServerAcceptor>,
) -> Result<String, JsErrorBox> {
    let local = listener
        .local_addr()
        .map(|address| address.to_string())
        .map_err(io_error)?;
    let id = next_listener_id();
    listeners().lock().expect("http2 listeners").insert(
        id,
        Arc::new(Listener {
            tcp: listener,
            acceptor: Mutex::new(acceptor.map(Arc::new)),
            local: local.clone(),
            notify: Arc::new(tokio::sync::Notify::new()),
        }),
    );
    Ok(format!("{id}\0{local}"))
}

#[op2(async(lazy), fast)]
#[string]
async fn op_http2_listen(#[string] host: String, port: u16) -> Result<String, JsErrorBox> {
    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .map_err(io_error)?;
    register_listener(listener, None)
}

#[op2(async(lazy), fast)]
#[string]
async fn op_http2_listen_secure(
    #[string] host: String,
    port: u16,
    #[string] cert_pem: String,
    #[string] key_pem: String,
    #[string] ca_pem: String,
    request_cert: bool,
) -> Result<String, JsErrorBox> {
    let acceptor = thinpack_node_tls::ServerAcceptor::new(
        &cert_pem,
        &key_pem,
        &ca_pem,
        request_cert,
        ALPN_H2,
    )?;
    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .map_err(io_error)?;
    register_listener(listener, Some(acceptor))
}

#[op2(fast)]
fn op_http2_listener_close(listener_id: u32) {
    let listener = listeners()
        .lock()
        .expect("http2 listeners")
        .remove(&listener_id);
    if let Some(listener) = listener {
        listener.notify.notify_waiters();
    }
}

#[op2(fast)]
fn op_http2_set_secure_context(
    listener_id: u32,
    #[string] cert_pem: &str,
    #[string] key_pem: &str,
    #[string] ca_pem: &str,
    request_cert: bool,
) -> Result<(), JsErrorBox> {
    let acceptor =
        thinpack_node_tls::ServerAcceptor::new(cert_pem, key_pem, ca_pem, request_cert, ALPN_H2)?;
    let listener = listeners()
        .lock()
        .expect("http2 listeners")
        .get(&listener_id)
        .cloned()
        .ok_or_else(|| JsErrorBox::new("Error", "EBADF\0listener closed"))?;
    let mut acceptor_slot = listener.acceptor.lock().expect("http2 listener acceptor");
    if acceptor_slot.is_none() {
        return Err(invalid("setSecureContext on a plaintext h2c listener"));
    }
    *acceptor_slot = Some(Arc::new(acceptor));
    Ok(())
}

#[op2]
#[string]
fn op_http2_listener_info(listener_id: u32) -> String {
    listeners()
        .lock()
        .expect("http2 listeners")
        .get(&listener_id)
        .map(|listener| listener.local.clone())
        .unwrap_or_default()
}

#[op2(async(lazy), fast)]
async fn op_http2_accept_session(listener_id: u32) -> Result<u32, JsErrorBox> {
    let listener = listeners()
        .lock()
        .expect("http2 listeners")
        .get(&listener_id)
        .cloned()
        .ok_or_else(|| JsErrorBox::new("Error", "EBADF\0listener closed"))?;
    let accepted = tokio::select! {
        accepted = listener.tcp.accept() => accepted,
        () = listener.notify.notified() => {
            return Err(JsErrorBox::new("Error", "ECANCELED\0listener closed"))
        }
    };
    let (mut tcp, _peer) = accepted.map_err(io_error)?;
    let _ = tcp.set_nodelay(true);
    let local = tcp
        .local_addr()
        .map(|address| address.to_string())
        .unwrap_or_default();
    let remote = tcp
        .peer_addr()
        .map(|address| address.to_string())
        .unwrap_or_default();
    let acceptor = listener
        .acceptor
        .lock()
        .expect("http2 listener acceptor")
        .clone();
    let alpn = if acceptor.is_some() { "h2" } else { "" };
    let transport: Box<dyn thinpack_node_tls::AsyncReadWrite> = if let Some(acceptor) = &acceptor {
        acceptor.accept(tcp).await?
    } else if reject_bad_preface(&mut tcp).await? {
        return Err(JsErrorBox::new(
            "Error",
            "ERR_HTTP2_ERROR\0Received bad client magic byte string",
        ));
    } else {
        Box::new(tcp)
    };
    let observer = Arc::new(Mutex::new(None));
    let (transport, inject) = injectable_transport(transport, Arc::clone(&observer));
    let mut connection = h2::server::handshake(transport)
        .await
        .map_err(http2_error)?;
    let ping = connection.ping_pong();
    let (session, commands) = new_session(
        None,
        format!("{local}\0{remote}\0{alpn}"),
        ping,
        Some(inject),
    );
    let session_id = insert_session(session);
    let events = session_of(session_id)?.events.clone();
    *observer.lock().expect("http2 goaway observer") = Some(events.clone());
    let driver = tokio::spawn(accept_requests(connection, session_id, events, commands));
    *session_of(session_id)?.driver.lock().expect("http2 driver") = Some(driver);
    Ok(session_id)
}

async fn reject_bad_preface(tcp: &mut TcpStream) -> Result<bool, JsErrorBox> {
    let mut prefix = [0u8; 8];
    let count = tcp.peek(&mut prefix).await.map_err(io_error)?;
    if count == 0 || H2_PREFACE.starts_with(&prefix[..count]) {
        return Ok(false);
    }
    tcp.write_all(&BAD_PREFACE_REPLY).await.map_err(io_error)?;
    tcp.shutdown().await.map_err(io_error)?;
    let mut drain = [0u8; 1024];
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while let Ok(count) = tcp.read(&mut drain).await {
            if count == 0 {
                break;
            }
        }
    })
    .await;
    Ok(true)
}

async fn accept_requests<S>(
    mut connection: ServerConnection<S, Bytes>,
    session_id: u32,
    events: mpsc::UnboundedSender<Vec<u8>>,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin,
{
    let mut commands_open = true;
    loop {
        let accepted = tokio::select! {
            accepted = connection.accept() => accepted,
            command = commands.recv(), if commands_open => {
                match command {
                    Some(SessionCommand::GoAway(reason)) if reason == Reason::NO_ERROR => {
                        connection.graceful_shutdown();
                    }
                    Some(SessionCommand::GoAway(reason)) => connection.abrupt_shutdown(reason),
                    None => commands_open = false,
                }
                continue;
            }
        };
        let Some(accepted) = accepted else { break };
        let (request, respond) = match accepted {
            Ok(pair) => pair,
            Err(error) => {
                if !error.is_go_away() {
                    send_event(&events, Vec::from([EVENT_ERROR]), &error.to_string());
                }
                break;
            }
        };
        let stream_id = next_stream_id();
        let (stream_events, incoming) = mpsc::unbounded_channel();
        let stream = Arc::new(Stream {
            session: session_id,
            events: stream_events.clone(),
            incoming: tokio::sync::Mutex::new(incoming),
            outbound: tokio::sync::Mutex::new(Some(Outbound::Pending(respond))),
        });
        if let Ok(session) = session_of(session_id) {
            session
                .streams
                .lock()
                .expect("http2 session streams")
                .insert(stream_id, stream.clone());
        }
        streams()
            .lock()
            .expect("http2 stream table")
            .insert(stream_id, stream);
        let mut event = Vec::from([EVENT_STREAM]);
        event.extend_from_slice(&stream_id.to_le_bytes());
        push_request_headers(&mut event, &request);
        if events.send(event).is_err() {
            break;
        }
        tokio::spawn(read_body(request.into_body(), stream_events));
    }
    let _ = events.send(Vec::from([EVENT_CLOSE]));
    let _ = events.send(Vec::new());
}

#[op2(async(lazy), fast)]
#[buffer]
async fn op_http2_session_event(session_id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let session = session_of(session_id)?;
    let mut incoming = session.incoming.lock().await;
    Ok(match incoming.recv().await {
        Some(event) => event.into_boxed_slice(),
        None => Box::new([]),
    })
}

#[op2(async(lazy), fast)]
async fn op_http2_ping(session_id: u32) -> Result<(), JsErrorBox> {
    let session = session_of(session_id)?;
    let mut ping = session.ping.lock().await;
    let ping = ping
        .as_mut()
        .ok_or_else(|| JsErrorBox::new("Error", "ERR_HTTP2_PING_CANCEL\0connection closed"))?;
    ping.ping(h2::Ping::opaque()).await.map_err(http2_error)?;
    Ok(())
}
