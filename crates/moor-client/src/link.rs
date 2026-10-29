//! The machine link: one WebSocket a machine agent dials to the runner. A few JSON text frames
//! prove who the agent is; then the socket carries yamux, whose streams later slices use.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

/// Where the runner accepts links.
pub const PATH: &str = "/machines/link";
/// The link protocol both ends speak; an agent on any other gets `update-required`.
pub const PROTOCOL: u32 = 1;
/// The sshsig namespace of the agent's proof, so no other signature by its key passes.
pub const NAMESPACE: &str = "moor-machine-link";
/// How often each end pings; three silent periods end the link.
pub const KEEPALIVE: Duration = Duration::from_secs(15);

/// The frames before the socket carries yamux, one JSON text message each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Frame {
    /// Agent: which machine dials, and what it speaks.
    Hello {
        /// The id enrollment gave the machine.
        machine_id: i64,
        /// The agent's [`PROTOCOL`].
        protocol: u32,
        /// The agent's release.
        version: String,
    },
    /// Runner: what the agent signs.
    Challenge {
        /// Fresh for this connection, hex.
        nonce: String,
    },
    /// Agent: its signature over [`signed`].
    Proof {
        /// Unix seconds when it signed.
        time: i64,
        /// An sshsig in PEM armor.
        signature: String,
    },
    /// Runner: the link is up.
    Welcome {
        /// The server's release.
        version: String,
    },
    /// Runner: the agent must become this release. Every protocol keeps this frame's shape,
    /// since an agent too old to speak anything else still has to read it.
    UpdateRequired {
        /// The server's release.
        version: String,
    },
    /// Runner: the agent is not let in.
    Refused {
        /// Why, for the agent's log.
        reason: String,
    },
}

/// The bytes the agent signs: the runner's nonce, the machine, and the time.
#[must_use]
pub fn signed(nonce: &str, machine_id: i64, time: i64) -> Vec<u8> {
    format!("{nonce}\n{machine_id}\n{time}").into_bytes()
}

/// Sends `frame` as a text message.
///
/// # Errors
/// The socket is closed.
pub async fn send<S>(ws: &mut WebSocketStream<S>, frame: &Frame) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let text = serde_json::to_string(frame).map_err(|e| e.to_string())?;
    ws.send(Message::text(text))
        .await
        .map_err(|e| format!("the link closed: {e}"))
}

/// The next frame, skipping pings.
///
/// # Errors
/// Nothing arrives `within`, the socket closes, or the message is not a frame.
pub async fn recv<S>(ws: &mut WebSocketStream<S>, within: Duration) -> Result<Frame, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let next = tokio::time::timeout_at(deadline, ws.next())
            .await
            .map_err(|_| "no answer in time".to_owned())?;
        match next {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text).map_err(|e| format!("not a link frame: {e}"));
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | None => return Err("the link closed".to_owned()),
            Some(Ok(_)) => return Err("not a link frame".to_owned()),
            Some(Err(e)) => return Err(format!("the link failed: {e}")),
        }
    }
}

pub use tokio_tungstenite::WebSocketStream;
pub use yamux::Mode;

/// The socket yamux runs on.
pub type Socket = Compat<DuplexStream>;

/// Spawns a pump that carries yamux's bytes over `ws` and pings every `keepalive`.
///
/// Returns the end yamux runs on. The pump closes it when the socket closes or fails, or
/// nothing arrives for three `keepalive` periods.
pub fn carry<S>(ws: WebSocketStream<S>, keepalive: Duration) -> Socket
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (near, far) = tokio::io::duplex(64 * 1024);
    tokio::spawn(pump(ws, far, keepalive));
    near.compat()
}

/// Moves bytes between `ws` and `far` until either ends or the far end goes quiet.
#[expect(
    clippy::cognitive_complexity,
    reason = "the count is select!'s expansion"
)]
async fn pump<S>(mut ws: WebSocketStream<S>, mut far: DuplexStream, keepalive: Duration)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut tick = tokio::time::interval_at(Instant::now() + keepalive, keepalive);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heard = Instant::now();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let open = tokio::select! {
            msg = ws.next() => {
                heard = Instant::now();
                deliver(msg, &mut far).await
            }
            n = far.read(&mut buf) => forward(&mut ws, n.ok().and_then(|n| buf.get(..n))).await,
            _ = tick.tick() => ping(&mut ws, heard.elapsed() <= keepalive * 3).await,
        };
        if !open {
            break;
        }
    }
    let _ = ws.close(None).await;
}

/// Sends yamux's `bytes` on; false once the link is over.
async fn forward<S>(ws: &mut WebSocketStream<S>, bytes: Option<&[u8]>) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let bytes = bytes.unwrap_or_default();
    !bytes.is_empty() && ws.send(Message::binary(bytes.to_vec())).await.is_ok()
}

/// Pings the far end while it is `alive`; false once the link is over.
async fn ping<S>(ws: &mut WebSocketStream<S>, alive: bool) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    alive && ws.send(Message::Ping(Vec::new().into())).await.is_ok()
}

/// Writes a binary message's bytes to `far`; false once the link is over.
async fn deliver(
    msg: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    far: &mut DuplexStream,
) -> bool {
    match msg {
        Some(Ok(Message::Binary(bytes))) => far.write_all(&bytes).await.is_ok(),
        Some(Ok(Message::Close(_)) | Err(_)) | None => false,
        Some(Ok(_)) => true,
    }
}

/// Runs yamux on `socket` until the link ends, turning away every stream the far end opens;
/// why it ended.
pub async fn hold(socket: Socket, mode: yamux::Mode) -> String {
    let mut conn = yamux::Connection::new(socket, yamux::Config::default(), mode);
    loop {
        match std::future::poll_fn(|cx| conn.poll_next_inbound(cx)).await {
            Some(Ok(stream)) => drop(stream),
            Some(Err(e)) => return format!("the link failed: {e}"),
            None => return "the link closed".to_owned(),
        }
    }
}

/// The link's address on `server`: `https` becomes `wss`, `http` becomes `ws`.
///
/// # Errors
/// `server` is neither.
pub fn url(server: &str) -> Result<String, String> {
    let server = server.trim_end_matches('/');
    let ws = if let Some(rest) = server.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = server.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(format!("{server} is not an http or https address"));
    };
    Ok(format!("{ws}{PATH}"))
}

/// A socket dialed to `server`'s link, verified against the bundled web roots.
pub type Dialed = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Dials `server`'s link.
///
/// # Errors
/// The address is bad, or the connection or upgrade fails.
pub async fn dial(server: &str) -> Result<Dialed, String> {
    let url = url(server)?;
    let tls = Connector::Rustls(Arc::new(crate::remote::tls_config()?));
    let (ws, _) = tokio_tungstenite::connect_async_tls_with_config(&url, None, false, Some(tls))
        .await
        .map_err(|e| format!("cannot reach {url}: {e}"))?;
    Ok(ws)
}

/// Takes the WebSocket upgrade on `stream`; a request for any path but [`PATH`] gets 404.
///
/// # Errors
/// The request is not an upgrade to [`PATH`].
#[expect(
    clippy::result_large_err,
    reason = "tungstenite fixes the callback's signature"
)]
pub async fn accept<S>(stream: S) -> Result<WebSocketStream<S>, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let only_link = |req: &Request, res: Response| {
        if req.uri().path() == PATH {
            return Ok(res);
        }
        let mut missing = ErrorResponse::new(Some("not found".to_owned()));
        *missing.status_mut() = StatusCode::NOT_FOUND;
        Err(missing)
    };
    tokio_tungstenite::accept_hdr_async(stream, only_link)
        .await
        .map_err(|e| format!("not a link: {e}"))
}
