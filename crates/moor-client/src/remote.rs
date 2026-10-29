//! The WebSocket transport: a signed-in dial to `<server>/front` that looks like a Unix
//! socket to everything above it.
//!
//! The server's binary messages carry the socket protocol's byte stream, cut anywhere. A
//! pump thread joins the WebSocket to one end of a socket pair and the client keeps the
//! other end, so frame reading, timeouts, `attach` and its half-close work as they do on a
//! controller socket. An empty message stands for the half-close in either direction.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::AUTHORIZATION;
use tungstenite::http::{HeaderValue, Uri};
use tungstenite::{client, Message, WebSocket};

use super::{Attempt, Client, ClientError};

/// Where the signed-in front lives on the server.
const FRONT_PATH: &str = "/front";

/// Time allowed to connect and complete the handshakes.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the pump waits on the server before it looks at the client's side.
const TICK: Duration = Duration::from_millis(10);

/// How long the pump waits on the client's side before it looks at the server again.
const LOCAL_TICK: Duration = Duration::from_millis(1);

/// Pump rounds, about [`TICK`] each, to wait for the server's close after the client ended
/// its side.
const CLOSE_TICKS: u32 = 1000;

/// Hands out the bearer token for each dial.
pub trait TokenSource: Send + Sync + fmt::Debug {
    /// A current access token for the server.
    ///
    /// # Errors
    /// Says why there is none, such as that nobody signed in.
    fn bearer(&self) -> Result<String, String>;
}

/// Where a client dials.
#[derive(Debug, Clone)]
pub(super) enum Link {
    /// A controller's Unix socket.
    Socket(PathBuf),
    /// A server reached over a signed-in WebSocket.
    Remote(Remote),
}

impl Link {
    /// Dials the controller, on its socket or through the server.
    pub(super) fn connect(&self) -> Attempt<UnixStream, ClientError> {
        let socket = match self {
            Self::Socket(socket) => socket,
            Self::Remote(remote) => return remote.connect(),
        };
        match UnixStream::connect(socket) {
            Ok(stream) => Attempt::Done(stream),
            Err(source) => {
                let kind = source.kind();
                let e = ClientError::Connect {
                    socket: socket.clone(),
                    source,
                };
                Attempt::connect_failed(kind, e)
            }
        }
    }
}

impl Client {
    /// A client for the moor server at `server` (an `https://` URL), signed in with the
    /// tokens `tokens` hands out. The requests, attaches and watches are the same as on a
    /// socket; the server decides who the client is.
    pub fn remote(server: impl Into<String>, tokens: Arc<dyn TokenSource>) -> Self {
        Self::linked(Link::Remote(Remote {
            server: server.into(),
            tokens,
        }))
    }
}

/// A server this client signs in to.
#[derive(Debug, Clone)]
pub(super) struct Remote {
    /// The server's base URL as given.
    pub(super) server: String,
    /// Where each dial gets its token.
    pub(super) tokens: Arc<dyn TokenSource>,
}

/// The TCP or TLS stream under the WebSocket.
enum Stream {
    /// `ws://`, accepted for a loopback server only.
    Plain(TcpStream),
    /// `wss://`.
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Stream {
    /// The TCP socket underneath.
    const fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(tcp) => tcp,
            Self::Tls(tls) => &tls.sock,
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.read(buf),
            Self::Tls(tls) => tls.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.write(buf),
            Self::Tls(tls) => tls.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(tcp) => tcp.flush(),
            Self::Tls(tls) => tls.flush(),
        }
    }
}

impl Remote {
    /// Dials the server and returns the client's end of the pumped connection. Nothing of
    /// the protocol has been sent, so a failure to connect or upgrade leaves nothing run.
    pub(super) fn connect(&self) -> Attempt<UnixStream, ClientError> {
        let token = match self.tokens.bearer() {
            Ok(token) => token,
            Err(why) => return Attempt::Failed(ClientError::Unauthorized(why)),
        };
        match self
            .upgrade(&token)
            .and_then(|ws| bridge(ws).map_err(Into::into))
        {
            Ok(near) => Attempt::Done(near),
            Err(Failure::Refused(why)) => Attempt::Failed(ClientError::Unauthorized(why)),
            Err(Failure::Down(why)) => Attempt::Unrun(ClientError::Remote(why)),
            Err(Failure::Bad(why)) => Attempt::Failed(ClientError::Remote(why)),
        }
    }

    /// Opens TCP and TLS and upgrades to the front's WebSocket, presenting `token`.
    fn upgrade(&self, token: &str) -> Result<WebSocket<Stream>, Failure> {
        let url = front_url(&self.server).map_err(Failure::Bad)?;
        let uri: Uri = url
            .parse()
            .map_err(|_| Failure::Bad("the server URL is not valid".to_owned()))?;
        let secure = uri.scheme_str() == Some("wss");
        let host = uri
            .host()
            .ok_or_else(|| Failure::Bad("the server URL has no host".to_owned()))?;
        if !secure && !loopback(host) {
            return Err(Failure::Bad(
                "refusing to send a sign-in over an unencrypted connection to a remote host"
                    .to_owned(),
            ));
        }
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        let tcp = dial(host, port).map_err(Failure::Down)?;
        let stream = if secure {
            Stream::Tls(Box::new(tls(host, tcp).map_err(Failure::Bad)?))
        } else {
            Stream::Plain(tcp)
        };
        let mut request = url
            .into_client_request()
            .map_err(|e| Failure::Bad(e.to_string()))?;
        let mut value = HeaderValue::try_from(format!("Bearer {token}"))
            .map_err(|_| Failure::Bad("the access token is not a valid header".to_owned()))?;
        value.set_sensitive(true);
        request.headers_mut().insert(AUTHORIZATION, value);
        let (ws, _) = client(request, stream).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp))
                if matches!(resp.status().as_u16(), 401 | 403) =>
            {
                Failure::Refused(format!(
                    "the server answered {}; run `moor-herdr login`",
                    resp.status()
                ))
            }
            tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp)) => {
                Failure::Down(format!("the server answered {}", resp.status()))
            }
            tungstenite::HandshakeError::Failure(inner) => Failure::Down(inner.to_string()),
            tungstenite::HandshakeError::Interrupted(_) => {
                Failure::Down("the upgrade timed out".to_owned())
            }
        })?;
        Ok(ws)
    }
}

/// Why a dial did not produce a connection.
enum Failure {
    /// The server refused the token.
    Refused(String),
    /// The server or network is not answering; a later dial may.
    Down(String),
    /// The dial cannot work as configured.
    Bad(String),
}

impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Self {
        Self::Bad(e.to_string())
    }
}

/// The WebSocket URL of the front of the server at `server`.
fn front_url(server: &str) -> Result<String, String> {
    let (scheme, rest) = server
        .split_once("://")
        .ok_or_else(|| format!("{server} is not a URL"))?;
    let scheme = match scheme {
        "https" | "wss" => "wss",
        "http" | "ws" => "ws",
        _ => return Err(format!("{server}: the scheme must be https")),
    };
    Ok(format!(
        "{scheme}://{}{FRONT_PATH}",
        rest.trim_end_matches('/')
    ))
}

/// True for a host that names this machine.
fn loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

/// Connects TCP to `host:port`.
fn dial(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs = (host.trim_start_matches('[').trim_end_matches(']'), port)
        .to_socket_addrs()
        .map_err(|e| format!("{host}: {e}"))?;
    let mut last = format!("{host}: no address");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(CONNECT_TIMEOUT))
                    .and_then(|()| tcp.set_write_timeout(Some(CONNECT_TIMEOUT)))
                    .map_err(|e| e.to_string())?;
                return Ok(tcp);
            }
            Err(e) => last = format!("{host}: {e}"),
        }
    }
    Err(last)
}

/// Starts TLS to `host` over `tcp`, verifying against the bundled web roots.
fn tls(host: &str, tcp: TcpStream) -> Result<StreamOwned<ClientConnection, TcpStream>, String> {
    let name = ServerName::try_from(host.to_owned())
        .map_err(|_| "the server's host is not a valid name".to_owned())?;
    let conn = ClientConnection::new(Arc::new(tls_config()?), name).map_err(|e| e.to_string())?;
    Ok(StreamOwned::new(conn, tcp))
}

/// TLS that verifies against the bundled web roots.
pub(crate) fn tls_config() -> Result<ClientConfig, String> {
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Ok(
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

/// Joins `ws` to a fresh socket pair and returns the client's end.
fn bridge(ws: WebSocket<Stream>) -> io::Result<UnixStream> {
    let (near, far) = UnixStream::pair()?;
    ws.get_ref().tcp().set_read_timeout(Some(TICK))?;
    far.set_read_timeout(Some(LOCAL_TICK))?;
    std::thread::Builder::new()
        .name("moor-front".to_owned())
        .spawn(move || pump(ws, &far))?;
    Ok(near)
}

/// True for the errors a read timeout produces.
fn timed_out(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// Which side of the pump is over, if either.
enum Step {
    /// Keep pumping.
    On,
    /// The connection is over.
    Over,
}

/// True for a WebSocket error that is only a read or write timeout.
fn only_timeout(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if timed_out(io))
}

/// Passes the server's next message, if any, to the client's end.
fn to_client(ws: &mut WebSocket<Stream>, mut far: &UnixStream) -> Step {
    match ws.read() {
        Ok(Message::Binary(bytes)) if bytes.is_empty() => {
            // The server will say no more; the client's reads end but its writes go on.
            match far.shutdown(std::net::Shutdown::Write) {
                Ok(()) => Step::On,
                Err(_) => Step::Over,
            }
        }
        Ok(Message::Binary(bytes)) if far.write_all(&bytes).is_ok() => Step::On,
        Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => Step::On,
        Err(e) if only_timeout(&e) => Step::On,
        _ => Step::Over,
    }
}

/// Passes what the client wrote, if anything, to the server. The client ending its writes
/// sends an empty message, which the server reads as that end shutting down, and sets
/// `closing`.
fn to_server(
    ws: &mut WebSocket<Stream>,
    mut far: &UnixStream,
    buf: &mut [u8],
    closing: &mut Option<u32>,
) -> Step {
    match far.read(buf) {
        Ok(0) => {
            *closing = Some(0);
            match ws.send(Message::Binary(Vec::new().into())) {
                Ok(()) => Step::On,
                Err(e) if only_timeout(&e) => Step::On,
                Err(_) => Step::Over,
            }
        }
        Ok(n) => {
            let bytes = buf.get(..n).unwrap_or_default().to_vec();
            match ws.send(Message::Binary(bytes.into())) {
                Ok(()) => Step::On,
                Err(e) if only_timeout(&e) => Step::On,
                Err(_) => Step::Over,
            }
        }
        Err(e) if timed_out(&e) => Step::On,
        Err(_) => Step::Over,
    }
}

/// Pumps bytes both ways until the server closes. When the client stops writing, tells the
/// server and keeps passing what it still says, for up to [`CLOSE_TICKS`].
fn pump(mut ws: WebSocket<Stream>, far: &UnixStream) {
    let mut buf = vec![0_u8; 16 * 1024];
    let mut closing: Option<u32> = None;
    loop {
        if matches!(to_client(&mut ws, far), Step::Over) {
            break;
        }
        let step = match closing.as_mut() {
            None => to_server(&mut ws, far, &mut buf, &mut closing),
            Some(ticks) if *ticks < CLOSE_TICKS => {
                *ticks += 1;
                Step::On
            }
            Some(_) => Step::Over,
        };
        let flushed = ws.flush();
        if matches!(step, Step::Over) || flushed.is_err_and(|e| !only_timeout(&e)) {
            break;
        }
    }
    let _ = far.shutdown(std::net::Shutdown::Both);
}
