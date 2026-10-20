//! The Socket Mode WebSocket loop: hands app mentions and button clicks to an intake, then
//! acknowledges their envelopes.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use moor_core::secret::Secret;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde_json::{json, Value};
use tungstenite::http::Uri;
use tungstenite::{client, Message, WebSocket};

use super::api::Api;

mod items;
mod parse;
pub(super) use items::{AfterAck, Kept, Slash, TokenSubmission};
use parse::{parse_click, parse_mention, parse_slash, parse_submission};

/// First wait after a failed or finished connection.
const MIN_BACKOFF: Duration = Duration::from_secs(1);

/// Longest wait between connection attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Time allowed to connect and complete the handshakes.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a read blocks before pending writes are flushed.
const READ_TICK: Duration = Duration::from_millis(500);

/// Consecutive silent read ticks (about 120 s) after which the connection is dead; Slack pings far more often.
const IDLE_TICKS: u32 = 240;

/// An app mention or thread reply received over Socket Mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Mention {
    /// Slack's event id, unique per delivered event.
    pub event_id: String,
    /// Channel the mention was posted in.
    pub channel: String,
    /// User who mentioned the app.
    pub user: String,
    /// Message text including the mention.
    pub text: String,
    /// Timestamp of the message.
    pub ts: String,
    /// Thread the message belongs to, if it is a reply.
    pub thread_ts: Option<String>,
    /// True for a plain thread reply, false for an app mention.
    pub(crate) reply: bool,
}

/// A button press on a prompt message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Click {
    /// Envelope id, used as the dedupe id.
    pub envelope_id: String,
    /// Channel holding the message.
    pub channel: String,
    /// User who pressed the button.
    pub user: String,
    /// Timestamp of the message carrying the button.
    pub message_ts: String,
    /// Thread the message belongs to, if known.
    pub thread_ts: Option<String>,
    /// 1-based option number.
    pub choice: u32,
}

/// What the socket thread hands to its intake.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Inbound {
    /// An app mention or thread reply.
    Mention(Mention),
    /// A button press.
    Click(Click),
    /// A slash command.
    Slash(Slash),
    /// A submitted login modal.
    Submission(TokenSubmission),
}

/// Takes each item before Slack gets the ack for its envelope.
pub(super) trait Intake {
    /// Keeps `item`, or says why its envelope gets no ack.
    fn take(&self, item: Inbound) -> Result<Kept, NotKept>;

    /// Does what a kept item asked for once its ack is sent.
    fn after_ack(&self, _then: AfterAck) {}
}

/// Why an intake did not keep an item.
#[derive(Debug)]
pub(super) enum NotKept {
    /// Nothing takes items any more, so the session ends.
    Gone,
    /// Keeping it failed; Slack sends an envelope that got no ack again.
    Failed,
}

/// Why a Socket Mode session ended abnormally; never carries the URL or a token.
#[derive(Debug, thiserror::Error)]
pub(super) enum SocketError {
    /// The WebSocket URL is not usable.
    #[error("socket url is not usable: {0}")]
    Url(String),
    /// Connecting or the handshake failed.
    #[error("socket connect failed: {0}")]
    Connect(String),
    /// The connection broke while running.
    #[error("socket failed: {0}")]
    Io(String),
    /// Slack stopped sending anything.
    #[error("socket went silent")]
    Idle,
    /// Nobody is listening for mentions any more.
    #[error("mention receiver is gone")]
    ReceiverGone,
}

/// A plain or TLS byte stream under the WebSocket.
enum Stream {
    /// `ws://`, used against a loopback fake.
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

/// What one envelope asks the read loop to do next.
enum Flow {
    /// Keep reading.
    Continue,
    /// Slack greeted us; the connection is healthy.
    Hello,
    /// Slack asked us to reconnect.
    Disconnect,
}

/// Reads envelopes from one connection until Slack disconnects or the socket closes.
#[cfg(test)]
pub(super) fn session(url: &str, intake: &dyn Intake) -> Result<(), SocketError> {
    session_tracked(url, intake, &mut false)
}

/// Runs forever, reconnecting with backoff; returns when `intake` is gone.
pub(super) fn run(api: &Api, app_token: &Secret, intake: &dyn Intake, sleep: &dyn Fn(Duration)) {
    let mut delay = MIN_BACKOFF;
    loop {
        if let Ok(url) = api.open_connection(app_token) {
            let mut healthy = false;
            if matches!(
                session_tracked(&url, intake, &mut healthy),
                Err(SocketError::ReceiverGone)
            ) {
                return;
            }
            if healthy {
                delay = MIN_BACKOFF;
            }
        }
        sleep(delay);
        delay = next_delay(delay);
    }
}

/// The wait that follows `delay`: doubled, capped at the maximum.
fn next_delay(delay: Duration) -> Duration {
    delay.saturating_mul(2).min(MAX_BACKOFF)
}

/// Reads envelopes from one connection, setting `healthy` once Slack's hello arrives.
fn session_tracked(url: &str, intake: &dyn Intake, healthy: &mut bool) -> Result<(), SocketError> {
    let mut ws = connect(url)?;
    let mut silent_ticks = 0_u32;
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                silent_ticks = 0;
                match handle_envelope(&text, &mut ws, intake)? {
                    Flow::Continue => {}
                    Flow::Hello => *healthy = true,
                    Flow::Disconnect => return Ok(()),
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => silent_ticks = 0,
            Err(tungstenite::Error::Io(e)) if is_timeout(&e) => {
                silent_ticks += 1;
                if silent_ticks > IDLE_TICKS {
                    return Err(SocketError::Idle);
                }
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(());
            }
            Err(e) => return Err(SocketError::Io(e.to_string())),
        }
        flush(&mut ws)?;
    }
}

/// True for the errors a read timeout produces.
fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// Writes out queued frames such as pongs; a busy socket is retried on the next tick.
fn flush(ws: &mut WebSocket<Stream>) -> Result<(), SocketError> {
    match ws.flush() {
        Ok(()) | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
            Ok(())
        }
        Err(tungstenite::Error::Io(e)) if is_timeout(&e) => Ok(()),
        Err(e) => Err(SocketError::Io(e.to_string())),
    }
}

/// Hands a mention or click in one JSON envelope to `intake`, then acknowledges the envelope.
/// An item the intake could not keep gets no ack, so Slack sends it again.
fn handle_envelope(
    text: &str,
    ws: &mut WebSocket<Stream>,
    intake: &dyn Intake,
) -> Result<Flow, SocketError> {
    let Ok(envelope) = serde_json::from_str::<Value>(text) else {
        return Ok(Flow::Continue);
    };
    let (flow, item) = match envelope.get("type").and_then(Value::as_str) {
        Some("hello") => (Flow::Hello, None),
        Some("disconnect") => (Flow::Disconnect, None),
        Some("events_api") => (
            Flow::Continue,
            parse_mention(&envelope).map(Inbound::Mention),
        ),
        Some("interactive") => (
            Flow::Continue,
            parse_click(&envelope)
                .map(Inbound::Click)
                .or_else(|| parse_submission(&envelope).map(Inbound::Submission)),
        ),
        Some("slash_commands") => (Flow::Continue, parse_slash(&envelope).map(Inbound::Slash)),
        _ => (Flow::Continue, None),
    };
    match item.map_or_else(|| Ok(Kept::default()), |item| intake.take(item)) {
        Ok(kept) => {
            ack(ws, &envelope, kept.ack)?;
            if let Some(then) = kept.then {
                flush(ws)?;
                intake.after_ack(then);
            }
            Ok(flow)
        }
        Err(NotKept::Failed) => Ok(flow),
        Err(NotKept::Gone) => Err(SocketError::ReceiverGone),
    }
}

/// Sends the ack for `envelope`, with `payload` when there is one, when it carries an
/// envelope id.
fn ack(
    ws: &mut WebSocket<Stream>,
    envelope: &Value,
    payload: Option<Value>,
) -> Result<(), SocketError> {
    let Some(id) = envelope.get("envelope_id").and_then(Value::as_str) else {
        return Ok(());
    };
    let frame = payload.map_or_else(
        || json!({"envelope_id": id}),
        |payload| json!({"envelope_id": id, "payload": payload}),
    );
    match ws.send(Message::text(frame.to_string())) {
        Ok(()) => Ok(()),
        Err(tungstenite::Error::Io(e)) if is_timeout(&e) => Ok(()),
        Err(e) => Err(SocketError::Io(e.to_string())),
    }
}

/// Opens the transport and completes the WebSocket handshake.
fn connect(url: &str) -> Result<WebSocket<Stream>, SocketError> {
    let uri: Uri = url
        .parse()
        .map_err(|_| SocketError::Url("not a valid URI".into()))?;
    let secure = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err(SocketError::Url("scheme must be ws or wss".into())),
    };
    let host = uri
        .host()
        .ok_or_else(|| SocketError::Url("no host".into()))?;
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let tcp = dial(host, port)?;
    let stream = if secure {
        Stream::Tls(Box::new(tls(host, tcp)?))
    } else {
        Stream::Plain(tcp)
    };
    let (ws, _) = client(url, stream).map_err(|e| SocketError::Connect(handshake_reason(&e)))?;
    ws.get_ref()
        .tcp()
        .set_read_timeout(Some(READ_TICK))
        .map_err(|e| SocketError::Connect(e.to_string()))?;
    Ok(ws)
}

/// A handshake failure described without the request URL.
fn handshake_reason<S: Read + Write>(
    e: &tungstenite::HandshakeError<tungstenite::ClientHandshake<S>>,
) -> String {
    match e {
        tungstenite::HandshakeError::Failure(inner) => inner.to_string(),
        tungstenite::HandshakeError::Interrupted(_) => "handshake timed out".into(),
    }
}

/// Opens TCP to `host:port` with the connect timeout applied to later reads and writes.
fn dial(host: &str, port: u16) -> Result<TcpStream, SocketError> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| SocketError::Connect(e.to_string()))?;
    let mut last = SocketError::Connect("no address".into());
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(CONNECT_TIMEOUT))
                    .and_then(|()| tcp.set_write_timeout(Some(CONNECT_TIMEOUT)))
                    .map_err(|e| SocketError::Connect(e.to_string()))?;
                return Ok(tcp);
            }
            Err(e) => last = SocketError::Connect(e.to_string()),
        }
    }
    Err(last)
}

/// Starts TLS to `host` over `tcp`, verifying against the bundled web roots.
fn tls(
    host: &str,
    tcp: TcpStream,
) -> Result<StreamOwned<ClientConnection, TcpStream>, SocketError> {
    let name = ServerName::try_from(host.to_owned())
        .map_err(|_| SocketError::Url("host is not a valid name".into()))?;
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| SocketError::Connect(e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
    let conn = ClientConnection::new(Arc::new(config), name)
        .map_err(|e| SocketError::Connect(e.to_string()))?;
    Ok(StreamOwned::new(conn, tcp))
}

#[cfg(test)]
mod tests;
