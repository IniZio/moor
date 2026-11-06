//! The machine link: one WebSocket a machine agent dials to the runner. A few JSON text frames
//! prove who the agent is; then the socket carries yamux, one stream per exec, hook or relay.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

pub mod broker;
pub mod lease;
pub mod ops;
pub mod status;
mod stream;
pub mod sweep;

pub use stream::{drive, Asked, Channel};

#[cfg(test)]
mod tests;

/// Where the runner accepts links.
pub const PATH: &str = "/machines/link";
/// The sshsig namespace of the agent's proof, so no other signature by its key passes.
pub const NAMESPACE: &str = "moor-machine-link";
/// How often each end pings; three silent periods end the link.
pub const KEEPALIVE: Duration = Duration::from_secs(15);

/// How often an agent hosting Boxes reports its host's memory.
pub const REPORT_EVERY: Duration = Duration::from_secs(10);

/// Agent: its host's memory now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// `MemAvailable` from `/proc/meminfo`, in MiB.
    pub mem_available_mib: u64,
    /// `MemTotal` from `/proc/meminfo`, in MiB.
    pub mem_total_mib: u64,
    /// The `some avg10` of `/proc/pressure/memory`; none when the host has no PSI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psi_some_avg10: Option<f64>,
    /// The `full avg10` of `/proc/pressure/memory`; none when the host has no PSI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psi_full_avg10: Option<f64>,
    /// MiB free on the filesystem holding the agent's home; none when it cannot tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free_mib: Option<u64>,
    /// Size of that filesystem, in MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_total_mib: Option<u64>,
    /// The memory the host's VMs hold, for each lease whose provider says; leases it cannot tell
    /// are left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boxes: Vec<BoxMemory>,
}

/// The host memory one lease's VM holds now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxMemory {
    /// `moor-<controller>-<id>`.
    pub slug: String,
    /// MiB of the host the VM holds; 0 when it is suspended to disk.
    pub host_memory_mib: u64,
    /// MiB of the host a suspended VM needs to resume; none when its provider does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_memory_mib: Option<u64>,
}

/// Each line of a stream the agent opens: the connection it accepted for a listen, or one of
/// its memory reports, which it keeps sending on that stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Opened {
    /// A connection for a listen; the stream then carries it.
    Accepted(crate::forward::Accepted),
    /// A memory report.
    Report {
        /// The report.
        report: Report,
    },
}

/// What a Box-hosting machine reports about itself in its hello.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// The most Boxes it will hold at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_boxes: Option<u32>,
    /// The disk, in GB, it sets aside for caches; reported, not enforced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_gb: Option<u32>,
    /// What its crabbox provider supports, such as `cache-volume`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// The host's total memory, in MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_mib: Option<u64>,
    /// The host's CPUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// The memory, in MiB, kept for the host rather than its Boxes; none means the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_mib: Option<u64>,
}

/// The frames before the socket carries yamux, one JSON text message each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Frame {
    /// Agent: which machine dials, and which release it runs.
    Hello {
        /// The id enrollment gave the machine.
        machine_id: i64,
        /// The agent's release.
        version: String,
        /// What the machine offers, when it hosts Boxes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offer: Option<Offer>,
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
    /// Runner: the agent must become this release. Every release keeps this frame's shape,
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
    /// Runner, beside yamux once the link is up: this runner is stopping, so dial again now and
    /// let this link's streams finish. An agent that does not know it skips it.
    Relink {},
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

/// A link that is up: the socket yamux runs on, and the frames sent beside it.
pub struct Carried {
    /// The end yamux runs on.
    pub socket: Socket,
    /// Frames to send the far end.
    pub tell: mpsc::Sender<Frame>,
    /// Frames the far end sent.
    pub heard: mpsc::Receiver<Frame>,
}

/// Spawns a pump that carries yamux's bytes over `ws` and pings every `keepalive`.
///
/// The pump closes the socket when `ws` closes or fails, or nothing arrives for three
/// `keepalive` periods.
pub fn carry<S>(ws: WebSocketStream<S>, keepalive: Duration) -> Carried
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (near, far) = tokio::io::duplex(64 * 1024);
    let (tell, told) = mpsc::channel(4);
    let (hear, heard) = mpsc::channel(4);
    tokio::spawn(pump(ws, far, keepalive, (told, hear)));
    Carried {
        socket: near.compat(),
        tell,
        heard,
    }
}

/// Moves bytes between `ws` and `far` until either ends or the far end goes quiet, sending the
/// frames told and passing on the frames heard.
#[expect(
    clippy::cognitive_complexity,
    reason = "the count is select!'s expansion"
)]
async fn pump<S>(
    mut ws: WebSocketStream<S>,
    mut far: DuplexStream,
    keepalive: Duration,
    (mut told, hear): (mpsc::Receiver<Frame>, mpsc::Sender<Frame>),
) where
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
                deliver(msg, &mut far, &hear).await
            }
            n = far.read(&mut buf) => forward(&mut ws, n.ok().and_then(|n| buf.get(..n))).await,
            Some(frame) = told.recv() => send(&mut ws, &frame).await.is_ok(),
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

/// Writes a binary message's bytes to `far` and passes a frame to `hear`; false once the link
/// is over. A text message that is no frame this release knows is skipped.
async fn deliver(
    msg: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    far: &mut DuplexStream,
    hear: &mpsc::Sender<Frame>,
) -> bool {
    match msg {
        Some(Ok(Message::Binary(bytes))) => far.write_all(&bytes).await.is_ok(),
        Some(Ok(Message::Text(text))) => {
            if let Ok(frame) = serde_json::from_str(&text) {
                let _ = hear.try_send(frame);
            }
            true
        }
        Some(Ok(Message::Close(_)) | Err(_)) | None => false,
        Some(Ok(_)) => true,
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
