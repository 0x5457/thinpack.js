use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Cursor, Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::{Arc, Mutex, OnceLock};

use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig, SignatureScheme};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use tokio_rustls::{TlsAcceptor, TlsConnector};

static CONNECT_CANCELS: thinpack_core::CancelTable = thinpack_core::CancelTable::new();
static READ_CANCELS: thinpack_core::CancelTable = thinpack_core::CancelTable::new();

pub(crate) const OPS: &[OpDecl] = &[
    op_tls_connect(),
    op_tls_connect_over(),
    op_tls_cancel_connect(),
    op_tls_cancel_read(),
    op_tls_read(),
    op_tls_write(),
    op_tls_shutdown(),
    op_tls_close(),
    op_tls_info(),
    op_tls_listen(),
    op_tls_accept(),
    op_tls_set_secure_context(),
    op_tls_listener_close(),
    op_tls_listener_info(),
    op_tls_stream_client(),
    op_tls_stream_feed(),
    op_tls_stream_eof(),
    op_tls_stream_write(),
    op_tls_stream_read(),
    op_tls_stream_output(),
    op_tls_stream_shutdown(),
    op_tls_stream_info(),
    op_tls_stream_close(),
];

struct Connection {
    read: tokio::sync::Mutex<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
    write: tokio::sync::Mutex<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>,
    shutdown: StdTcpStream,
    alpn: String,
    authorized: bool,
}

fn read_cancelled() -> &'static Mutex<HashSet<u32>> {
    static CANCELLED: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    CANCELLED.get_or_init(|| Mutex::new(HashSet::new()))
}

#[op2(fast)]
fn op_tls_cancel_connect(attempt: u32) {
    let _ = CONNECT_CANCELS.cancel(attempt);
}

#[op2(fast)]
fn op_tls_cancel_read(id: u32) {
    read_cancelled()
        .lock()
        .expect("tls read cancelled")
        .insert(id);
    let _ = READ_CANCELS.cancel(id);
}

fn table() -> &'static Mutex<HashMap<u32, Arc<Connection>>> {
    static TABLE: OnceLock<Mutex<HashMap<u32, Arc<Connection>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

static CONNECTIONS: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

fn tls_error(code: &str, detail: impl std::fmt::Display) -> JsErrorBox {
    JsErrorBox::new("Error", format!("{code}\0{detail}"))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "`io::Error` is a `map_err` sink: taking a reference would only force a closure at every call site"
)]
fn io_error(error: io::Error) -> JsErrorBox {
    let code = error
        .raw_os_error()
        .map_or_else(|| "EIO".to_owned(), |errno| format!("-{errno}"));
    JsErrorBox::new("Error", format!("{code}\0{error}"))
}

#[derive(Debug)]
struct NoVerification;

impl ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn parse_certs(pem: &str) -> Result<Vec<CertificateDer<'static>>, JsErrorBox> {
    rustls_pemfile::certs(&mut Cursor::new(pem.as_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| tls_error("ERR_TLS_INVALID_CERT", error))
}

const fn rustls_error_code(error: &rustls::Error) -> &'static str {
    match error {
        rustls::Error::InvalidCertificate(certificate) => match certificate {
            rustls::CertificateError::NotValidForName
            | rustls::CertificateError::NotValidForNameContext { .. } => {
                "ERR_TLS_CERT_ALTNAME_INVALID"
            }
            rustls::CertificateError::UnknownIssuer => "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
            rustls::CertificateError::Expired | rustls::CertificateError::ExpiredContext { .. } => {
                "CERT_HAS_EXPIRED"
            }
            rustls::CertificateError::NotValidYet
            | rustls::CertificateError::NotValidYetContext { .. } => "CERT_NOT_YET_VALID",
            rustls::CertificateError::Revoked => "CERT_REVOKED",
            _ => "ERR_TLS_HANDSHAKE_FAILURE",
        },
        _ => "ERR_TLS_HANDSHAKE_FAILURE",
    }
}

fn rustls_failure(error: rustls::Error) -> JsErrorBox {
    tls_error(rustls_error_code(&error), error)
}

fn handshake_error(error: io::Error) -> JsErrorBox {
    if error.raw_os_error().is_some() {
        return io_error(error);
    }
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<rustls::Error>())
        .cloned()
        .map_or_else(
            || tls_error("ERR_TLS_HANDSHAKE_FAILURE", error),
            rustls_failure,
        )
}

fn insert_connection<S>(tls: S, shutdown: StdTcpStream, alpn: String, authorized: bool) -> u32
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (read, write) = tokio::io::split(tls);
    let id = CONNECTIONS.next();
    table().lock().expect("tls table").insert(
        id,
        Arc::new(Connection {
            read: tokio::sync::Mutex::new(Box::new(read)),
            write: tokio::sync::Mutex::new(Box::new(write)),
            shutdown,
            alpn,
            authorized,
        }),
    );
    id
}

fn client_config(
    ca_pem: &str,
    cert_pem: &str,
    key_pem: &str,
    alpn: &str,
    reject_unauthorized: bool,
) -> Result<Arc<ClientConfig>, JsErrorBox> {
    let mut roots = RootCertStore::empty();
    if ca_pem.trim().is_empty() {
        // System trust stores routinely hold entries rustls cannot consume (legacy
        // signatures, stray files); loading them is best-effort, mirroring Node's
        // use of the platform store, where one bad entry must not disable TLS.
        // A user-supplied `ca`, by contrast, must parse or the connect fails loudly
        // below.
        let native = rustls_native_certs::load_native_certs();
        for certificate in native.certs {
            let _ = roots.add(certificate);
        }
    } else {
        for certificate in parse_certs(ca_pem)? {
            roots
                .add(certificate)
                .map_err(|error| tls_error("ERR_TLS_INVALID_CERT", error))?;
        }
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| tls_error("ERR_TLS_CONFIG", error))?
        .with_root_certificates(roots);

    let mut config = if cert_pem.trim().is_empty() {
        builder.with_no_client_auth()
    } else {
        let certificates = parse_certs(cert_pem)?;
        let key = rustls_pemfile::private_key(&mut Cursor::new(key_pem.as_bytes()))
            .map_err(|error| tls_error("ERR_TLS_INVALID_KEY", error))?
            .ok_or_else(|| tls_error("ERR_TLS_INVALID_KEY", "no private key in key_pem"))?;
        builder
            .with_client_auth_cert(certificates, key)
            .map_err(|error| tls_error("ERR_TLS_CONFIG", error))?
    };

    config.alpn_protocols = alpn
        .split(',')
        .filter(|protocol| !protocol.is_empty())
        .map(|protocol| protocol.as_bytes().to_vec())
        .collect();

    if !reject_unauthorized {
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoVerification));
    }

    Ok(Arc::new(config))
}

#[op2(async(lazy), fast)]
async fn op_tls_connect(
    #[string] host: String,
    port: u16,
    #[string] servername: String,
    #[string] ca_pem: String,
    #[string] cert_pem: String,
    #[string] key_pem: String,
    #[string] alpn: String,
    reject_unauthorized: bool,
    attempt: u32,
) -> Result<u32, JsErrorBox> {
    let cancelled = CONNECT_CANCELS.register(attempt);
    let result = tokio::select! {
        result = connect_tls(host, port, servername, ca_pem, cert_pem, key_pem, alpn, reject_unauthorized) => result,
        _ = cancelled => Err(tls_error("ECANCELED", "connect aborted")),
    };
    CONNECT_CANCELS.remove(attempt);
    result
}

#[expect(
    clippy::too_many_arguments,
    reason = "the op ABI passes tls.connect's option bag positionally"
)]
async fn connect_tls(
    host: String,
    port: u16,
    servername: String,
    ca_pem: String,
    cert_pem: String,
    key_pem: String,
    alpn: String,
    reject_unauthorized: bool,
) -> Result<u32, JsErrorBox> {
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(io_error)?;
    start_tls(
        tcp,
        servername,
        ca_pem,
        cert_pem,
        key_pem,
        alpn,
        reject_unauthorized,
    )
    .await
}

#[op2(async(lazy), fast)]
async fn op_tls_connect_over(
    tcp_id: u32,
    #[string] servername: String,
    #[string] ca_pem: String,
    #[string] cert_pem: String,
    #[string] key_pem: String,
    #[string] alpn: String,
    reject_unauthorized: bool,
    attempt: u32,
) -> Result<u32, JsErrorBox> {
    let tcp = thinpack_node_net::take_stream(tcp_id).map_err(|error| match error {
        thinpack_node_net::TakeStreamError::NotFound => tls_error("EBADF", "unknown connection"),
        thinpack_node_net::TakeStreamError::Busy => tls_error("EBUSY", "connection is busy"),
        thinpack_node_net::TakeStreamError::NotTcp => {
            tls_error("ERR_NOT_IMPLEMENTED", "TLS over a Unix domain socket")
        }
        thinpack_node_net::TakeStreamError::Io(error) => io_error(error),
    })?;
    let cancelled = CONNECT_CANCELS.register(attempt);
    let result = tokio::select! {
        result = start_tls(tcp, servername, ca_pem, cert_pem, key_pem, alpn, reject_unauthorized) => result,
        _ = cancelled => Err(tls_error("ECANCELED", "connect aborted")),
    };
    CONNECT_CANCELS.remove(attempt);
    result
}

async fn start_tls(
    tcp: TcpStream,
    servername: String,
    ca_pem: String,
    cert_pem: String,
    key_pem: String,
    alpn: String,
    reject_unauthorized: bool,
) -> Result<u32, JsErrorBox> {
    let config = client_config(&ca_pem, &cert_pem, &key_pem, &alpn, reject_unauthorized)?;
    let connector = TlsConnector::from(config);
    let _ = tcp.set_nodelay(true);
    let std_stream = tcp.into_std().map_err(io_error)?;
    let shutdown = std_stream.try_clone().map_err(io_error)?;
    let tcp = TcpStream::from_std(std_stream).map_err(io_error)?;

    let dns_name = ServerName::try_from(servername.as_str())
        .map_err(|error| tls_error("ERR_TLS_INVALID_SERVERNAME", error))?
        .to_owned();
    let tls = connector
        .connect(dns_name, tcp)
        .await
        .map_err(handshake_error)?;

    let negotiated = tls
        .get_ref()
        .1
        .alpn_protocol()
        .map(|protocol| String::from_utf8_lossy(protocol).into_owned())
        .unwrap_or_default();
    Ok(insert_connection(
        tls,
        shutdown,
        negotiated,
        reject_unauthorized,
    ))
}

#[op2(async(lazy), fast)]
#[buffer]
async fn op_tls_read(id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let connection = connection_of(id)?;
    let mut buffer = vec![0u8; 65536];
    if read_cancelled()
        .lock()
        .expect("tls read cancelled")
        .remove(&id)
    {
        return Err(tls_error("ECANCELED", "read aborted"));
    }
    let mut read = connection.read.lock().await;
    let cancelled = READ_CANCELS.register(id);
    let result = tokio::select! {
        result = read.read(&mut buffer) => Some(result),
        _ = cancelled => None,
    };
    READ_CANCELS.remove(id);
    read_cancelled()
        .lock()
        .expect("tls read cancelled")
        .remove(&id);
    let count = match result.ok_or_else(|| tls_error("ECANCELED", "read aborted"))? {
        Ok(count) => count,
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => 0,
        Err(error) => return Err(io_error(error)),
    };
    buffer.truncate(count);
    Ok(buffer.into_boxed_slice())
}

#[op2(async(lazy), fast)]
async fn op_tls_write(id: u32, #[buffer(copy)] data: Vec<u8>) -> Result<u32, JsErrorBox> {
    let connection = connection_of(id)?;
    let mut write = connection.write.lock().await;
    write.write_all(&data).await.map_err(io_error)?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "op buffers come from V8 typed arrays, whose length stays below `u32::MAX`"
    )]
    let written = data.len() as u32;
    Ok(written)
}

#[op2(async(lazy), fast)]
async fn op_tls_shutdown(id: u32) -> Result<(), JsErrorBox> {
    let connection = connection_of(id)?;
    let mut write = connection.write.lock().await;
    write.shutdown().await.map_err(io_error)
}

#[op2(fast)]
fn op_tls_close(id: u32) {
    let connection = table().lock().expect("tls table").remove(&id);
    if let Some(connection) = connection {
        let _ = connection.shutdown.shutdown(std::net::Shutdown::Both);
    }
}

#[op2]
#[string]
fn op_tls_info(id: u32) -> String {
    table().lock().expect("tls table").get(&id).map_or_else(
        || "\0".to_owned(),
        |connection| {
            format!(
                "{}\0{}",
                connection.alpn,
                if connection.authorized { "1" } else { "0" }
            )
        },
    )
}

fn connection_of(id: u32) -> Result<Arc<Connection>, JsErrorBox> {
    table()
        .lock()
        .expect("tls table")
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| JsErrorBox::new("Error", "EBADF\0connection closed"))
}

/// How the peer's TLS read side ended. A `close_notify` is a clean half-close;
/// a transport EOF without one is reported separately instead of being
/// presented as an orderly shutdown.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PeerClose {
    Open,
    Clean,
    Unclean,
}

/// A client TLS session whose ciphertext is moved by JavaScript over any byte
/// `Duplex`. Rust owns the protocol state; JS owns the transport and its write
/// completions, so a TLS write callback fires only after the encrypted bytes
/// have been accepted by the underlying stream.
struct StreamConnection {
    connection: rustls::ClientConnection,
    plaintext: VecDeque<u8>,
    authorized: bool,
    peer_close: PeerClose,
    eof: bool,
    local_closed: bool,
}

fn streams() -> &'static Mutex<HashMap<u32, Arc<Mutex<StreamConnection>>>> {
    static STREAMS: OnceLock<Mutex<HashMap<u32, Arc<Mutex<StreamConnection>>>>> = OnceLock::new();
    STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

static STREAM_CONNECTIONS: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

fn stream_of(id: u32) -> Result<Arc<Mutex<StreamConnection>>, JsErrorBox> {
    streams()
        .lock()
        .expect("tls streams")
        .get(&id)
        .cloned()
        .ok_or_else(|| tls_error("EBADF", "connection closed"))
}

/// Moves rustls' newly decrypted application data into the read queue. A raw
/// EOF is recorded separately from a peer `close_notify` so the caller can
/// report the difference instead of treating both as a clean close. rustls only
/// buffers 16 KiB of received plaintext, so this runs after every feed even
/// when JavaScript has paused its readable side; the queue is not a fixed-size
/// ingress buffer.
fn stream_plaintext(stream: &mut StreamConnection) -> Result<(), JsErrorBox> {
    loop {
        let mut buffer = [0u8; 16384];
        match stream.connection.reader().read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => stream.plaintext.extend(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                if stream.peer_close != PeerClose::Clean {
                    stream.peer_close = PeerClose::Unclean;
                }
                break;
            }
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

#[op2(fast)]
fn op_tls_stream_client(
    #[string] servername: &str,
    #[string] ca_pem: &str,
    #[string] cert_pem: &str,
    #[string] key_pem: &str,
    #[string] alpn: &str,
    reject_unauthorized: bool,
) -> Result<u32, JsErrorBox> {
    let config = client_config(ca_pem, cert_pem, key_pem, alpn, reject_unauthorized)?;
    let name = ServerName::try_from(servername)
        .map_err(|error| tls_error("ERR_TLS_INVALID_SERVERNAME", error))?
        .to_owned();
    let connection = rustls::ClientConnection::new(config, name)
        .map_err(|error| tls_error("ERR_TLS_CONFIG", error))?;
    let stream = StreamConnection {
        connection,
        plaintext: VecDeque::new(),
        authorized: reject_unauthorized,
        peer_close: PeerClose::Open,
        eof: false,
        local_closed: false,
    };
    let id = STREAM_CONNECTIONS.next();
    streams()
        .lock()
        .expect("tls streams")
        .insert(id, Arc::new(Mutex::new(stream)));
    Ok(id)
}

#[op2(fast)]
fn op_tls_stream_feed(id: u32, #[buffer] data: &[u8]) -> Result<(), JsErrorBox> {
    let stream = stream_of(id)?;
    let mut stream = stream.lock().expect("tls stream");
    let mut input = data;
    while !input.is_empty() {
        let read = stream.connection.read_tls(&mut input).map_err(io_error)?;
        if read == 0 {
            return Err(tls_error("EIO", "TLS input was not consumed"));
        }
        let state = stream
            .connection
            .process_new_packets()
            .map_err(rustls_failure)?;
        if state.peer_has_closed() {
            stream.peer_close = PeerClose::Clean;
        }
        stream_plaintext(&mut stream)?;
    }
    Ok(())
}

#[op2(fast)]
fn op_tls_stream_eof(id: u32) -> Result<(), JsErrorBox> {
    let stream = stream_of(id)?;
    let mut stream = stream.lock().expect("tls stream");
    if stream.eof {
        return Ok(());
    }
    stream.eof = true;
    let mut empty: &[u8] = &[];
    stream.connection.read_tls(&mut empty).map_err(io_error)?;
    let state = stream
        .connection
        .process_new_packets()
        .map_err(rustls_failure)?;
    if state.peer_has_closed() {
        stream.peer_close = PeerClose::Clean;
    }
    stream_plaintext(&mut stream)
}

#[op2(fast)]
fn op_tls_stream_write(id: u32, #[buffer] data: &[u8]) -> Result<(), JsErrorBox> {
    let stream = stream_of(id)?;
    let mut stream = stream.lock().expect("tls stream");
    let mut input = data;
    while !input.is_empty() {
        let written = stream
            .connection
            .writer()
            .write(input)
            .map_err(handshake_error)?;
        if written == 0 {
            return Err(tls_error("ERR_TLS_WRITE", "the TLS send buffer is full"));
        }
        input = &input[written..];
    }
    Ok(())
}

#[op2]
#[buffer]
fn op_tls_stream_read(id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let stream = stream_of(id)?;
    let mut stream = stream.lock().expect("tls stream");
    if stream.plaintext.is_empty() {
        if stream.peer_close == PeerClose::Unclean {
            stream.peer_close = PeerClose::Open;
            return Err(tls_error(
                "ERR_SSL_UNEXPECTED_EOF",
                "peer closed the TLS connection without close_notify",
            ));
        }
        return Ok(Box::new([]));
    }
    let count = stream.plaintext.len().min(65536);
    Ok(stream
        .plaintext
        .drain(..count)
        .collect::<Vec<_>>()
        .into_boxed_slice())
}

#[op2]
#[buffer]
fn op_tls_stream_output(id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let stream = stream_of(id)?;
    let mut stream = stream.lock().expect("tls stream");
    let mut output = Vec::new();
    let count = stream.connection.write_tls(&mut output).map_err(io_error)?;
    output.truncate(count);
    Ok(output.into_boxed_slice())
}

#[op2(fast)]
fn op_tls_stream_shutdown(id: u32) -> Result<(), JsErrorBox> {
    let stream = stream_of(id)?;
    let mut stream = stream.lock().expect("tls stream");
    if !stream.local_closed {
        stream.connection.send_close_notify();
        stream.local_closed = true;
    }
    Ok(())
}

#[op2]
#[string]
fn op_tls_stream_info(id: u32) -> String {
    streams().lock().expect("tls streams").get(&id).map_or_else(
        || "\0\0\0\0".to_owned(),
        |stream| {
            let stream = stream.lock().expect("tls stream");
            let alpn = stream
                .connection
                .alpn_protocol()
                .map_or_else(String::new, |protocol| {
                    String::from_utf8_lossy(protocol).into_owned()
                });
            let handshaking = stream.connection.is_handshaking();
            format!(
                "{alpn}\0{}\0{}\0{}",
                if !handshaking && stream.authorized {
                    "1"
                } else {
                    "0"
                },
                if handshaking { "1" } else { "0" },
                if stream.peer_close == PeerClose::Clean {
                    "1"
                } else {
                    "0"
                },
            )
        },
    )
}

#[op2(fast)]
fn op_tls_stream_close(id: u32) {
    let _ = streams().lock().expect("tls streams").remove(&id);
}

struct TlsListen {
    listener: Arc<TcpListener>,
    config: Mutex<Arc<ServerConfig>>,
    notify: Arc<tokio::sync::Notify>,
}

fn listeners() -> &'static Mutex<HashMap<u32, Arc<TlsListen>>> {
    static LISTENERS: OnceLock<Mutex<HashMap<u32, Arc<TlsListen>>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(HashMap::new()))
}

static LISTENERS: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

fn server_config(
    cert_pem: &str,
    key_pem: &str,
    ca_pem: &str,
    request_cert: bool,
    alpn: &str,
) -> Result<Arc<ServerConfig>, JsErrorBox> {
    let certificates = parse_certs(cert_pem)?;
    let key = rustls_pemfile::private_key(&mut Cursor::new(key_pem.as_bytes()))
        .map_err(|error| tls_error("ERR_TLS_INVALID_KEY", error))?
        .ok_or_else(|| tls_error("ERR_TLS_INVALID_KEY", "no private key in key_pem"))?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| tls_error("ERR_TLS_CONFIG", error))?;
    let builder = if request_cert && !ca_pem.trim().is_empty() {
        let mut roots = RootCertStore::empty();
        for certificate in parse_certs(ca_pem)? {
            roots
                .add(certificate)
                .map_err(|error| tls_error("ERR_TLS_INVALID_CERT", error))?;
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|error| tls_error("ERR_TLS_CONFIG", error))?;
        builder.with_client_cert_verifier(verifier)
    } else {
        builder.with_no_client_auth()
    };
    let mut config = builder
        .with_single_cert(certificates, key)
        .map_err(|error| tls_error("ERR_TLS_CONFIG", error))?;
    config.alpn_protocols = alpn
        .split(',')
        .filter(|protocol| !protocol.is_empty())
        .map(|protocol| protocol.as_bytes().to_vec())
        .collect();
    Ok(Arc::new(config))
}

#[op2(async(lazy), fast)]
#[string]
async fn op_tls_listen(
    #[string] host: String,
    port: u16,
    #[string] cert_pem: String,
    #[string] key_pem: String,
    #[string] ca_pem: String,
    request_cert: bool,
    #[string] alpn: String,
) -> Result<String, JsErrorBox> {
    let config = server_config(&cert_pem, &key_pem, &ca_pem, request_cert, &alpn)?;
    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .map_err(io_error)?;
    let address = listener.local_addr().map_err(io_error)?;
    let id = LISTENERS.next();
    listeners().lock().expect("tls listeners").insert(
        id,
        Arc::new(TlsListen {
            listener: Arc::new(listener),
            config: Mutex::new(config),
            notify: Arc::new(tokio::sync::Notify::new()),
        }),
    );
    Ok(format!("{id}\0{address}"))
}

#[op2(async(lazy), fast)]
async fn op_tls_accept(listener_id: u32) -> Result<u32, JsErrorBox> {
    let listener = listeners()
        .lock()
        .expect("tls listeners")
        .get(&listener_id)
        .cloned()
        .ok_or_else(|| tls_error("EBADF", "listener closed"))?;

    let accepted = tokio::select! {
        result = listener.listener.accept() => result,
        () = listener.notify.notified() => return Err(tls_error("ECANCELED", "listener closed")),
    };
    let (tcp, _peer) = accepted.map_err(io_error)?;
    let _ = tcp.set_nodelay(true);

    let std_stream = tcp.into_std().map_err(io_error)?;
    let shutdown = std_stream.try_clone().map_err(io_error)?;
    let tcp = TcpStream::from_std(std_stream).map_err(io_error)?;
    let config = listener.config.lock().expect("tls listener config").clone();
    let acceptor = TlsAcceptor::from(config);
    let tls = acceptor.accept(tcp).await.map_err(handshake_error)?;

    let negotiated = tls
        .get_ref()
        .1
        .alpn_protocol()
        .map(|protocol| String::from_utf8_lossy(protocol).into_owned())
        .unwrap_or_default();
    let authorized = tls.get_ref().1.peer_certificates().is_some();
    Ok(insert_connection(tls, shutdown, negotiated, authorized))
}

#[op2(fast)]
fn op_tls_set_secure_context(
    listener_id: u32,
    #[string] cert_pem: &str,
    #[string] key_pem: &str,
    #[string] ca_pem: &str,
    request_cert: bool,
    #[string] alpn: &str,
) -> Result<(), JsErrorBox> {
    let config = server_config(cert_pem, key_pem, ca_pem, request_cert, alpn)?;
    let listener = listeners()
        .lock()
        .expect("tls listeners")
        .get(&listener_id)
        .cloned()
        .ok_or_else(|| tls_error("EBADF", "listener closed"))?;
    *listener.config.lock().expect("tls listener config") = config;
    Ok(())
}

#[op2(fast)]
fn op_tls_listener_close(listener_id: u32) {
    let Some(listener) = listeners()
        .lock()
        .expect("tls listeners")
        .remove(&listener_id)
    else {
        return;
    };
    listener.notify.notify_waiters();
}

#[op2]
#[string]
fn op_tls_listener_info(listener_id: u32) -> String {
    listeners()
        .lock()
        .expect("tls listeners")
        .get(&listener_id)
        .and_then(|listener| listener.listener.local_addr().ok())
        .map(|address| address.to_string())
        .unwrap_or_default()
}

pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> AsyncReadWrite for T {}

#[derive(Debug)]
pub enum TakeError {
    NotFound,
    Busy,
}

/// Takes the TLS connection out of the connection table, so HTTP/2 can drive the
/// already negotiated stream.
///
/// # Errors
///
/// Returns [`TakeError::NotFound`] for an unknown id and [`TakeError::Busy`] while
/// another op still holds the connection.
///
/// # Panics
///
/// Panics if the connection table lock is poisoned.
pub fn take_connection(id: u32) -> Result<Box<dyn AsyncReadWrite>, TakeError> {
    let entry = table()
        .lock()
        .expect("tls table")
        .remove(&id)
        .ok_or(TakeError::NotFound)?;
    let connection = match Arc::try_unwrap(entry) {
        Ok(connection) => connection,
        Err(entry) => {
            table().lock().expect("tls table").insert(id, entry);
            return Err(TakeError::Busy);
        }
    };
    let Connection { read, write, .. } = connection;
    Ok(Box::new(tokio::io::join(
        read.into_inner(),
        write.into_inner(),
    )))
}

pub struct ServerAcceptor {
    acceptor: TlsAcceptor,
}

impl std::fmt::Debug for ServerAcceptor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `TlsAcceptor` has no `Debug` impl of its own.
        formatter
            .debug_struct("ServerAcceptor")
            .finish_non_exhaustive()
    }
}

impl ServerAcceptor {
    /// Builds an acceptor from the PEM material an HTTP/2 secure server needs.
    ///
    /// # Errors
    ///
    /// Returns an error when the certificate or key PEM cannot be parsed or the
    /// resulting server config is rejected by rustls.
    pub fn new(
        cert_pem: &str,
        key_pem: &str,
        ca_pem: &str,
        request_cert: bool,
        alpn: &str,
    ) -> Result<Self, JsErrorBox> {
        let config = server_config(cert_pem, key_pem, ca_pem, request_cert, alpn)?;
        Ok(Self {
            acceptor: TlsAcceptor::from(config),
        })
    }

    /// Runs the server handshake on an accepted TCP stream.
    ///
    /// # Errors
    ///
    /// Returns the TLS error or IO error that ended the handshake, mapped to the same
    /// `code\0detail` shape the handshake ops report.
    pub async fn accept(&self, tcp: TcpStream) -> Result<Box<dyn AsyncReadWrite>, JsErrorBox> {
        let _ = tcp.set_nodelay(true);
        let tls = self.acceptor.accept(tcp).await.map_err(handshake_error)?;
        Ok(Box::new(tls))
    }
}
