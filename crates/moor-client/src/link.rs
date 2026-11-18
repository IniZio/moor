//! The machine link: one WebSocket a machine agent dials to the runner. A few JSON text frames
//! prove who the agent is; then the socket carries yamux, one stream per exec, hook or relay.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

pub mod boxes;
pub mod broker;
pub mod chan;
mod dial;
pub mod lease;
pub mod ops;
mod pump;
pub mod reach;
pub mod status;
mod stream;
pub mod sweep;

pub use dial::{dial, dial_found, Dialed};
pub use pump::{carry, Carried, Socket};
pub use stream::{drive, Asked, Channel};

#[cfg(test)]
mod tests;

/// Where the runner accepts links.
pub const PATH: &str = "/machines/link";
/// The sshsig namespace of the agent's proof, so no other signature by its key passes.
pub const NAMESPACE: &str = "moor-machine-link";
/// How often each end pings. An end warns after two silent periods and ends the link after
/// three, well before a tunnel drops an idle socket.
pub const KEEPALIVE: Duration = Duration::from_secs(5);

/// How long a dial may take to connect and upgrade, so a dropped SYN fails instead of hanging.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// The memory the host's VMs hold, for each lease whoseprovider says; leases it cannot tell
    /// are left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boxes: Vec<BoxMemory>,
    /// Each Box the machine provides, as [`boxes::BoxReport`] JSON. Carried raw, so a runner
    /// forwards what a newer agent says without knowing it, and one entry it cannot read loses
    /// only that entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub box_health: Vec<serde_json::Value>,
}

/// The host memory one lease's VM holds now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxMemory {
    /// `moor-<controller>-<id>`.
    pub slug: String,
    /// MiB of the host the VM holds; 0 when it is suspended to disk.
    pub host_memory_mib: u64,
    /// MiB of the host a suspended VM needs to resume; none when itsprovider does not say.
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
    /// The disk, in GB, it sets aside for caches; reported, not enforced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_gb: Option<u32>,
    /// What its provider supports, such as `cache-volume`.
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
    /// That it provides Boxes itself, and how to speak to it about them; none for a machine that
    /// does not, or an agent from before the Box protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boxes: Option<boxes::BoxOffer>,
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
        offer: Option<Box<Offer>>,
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
    /// Runner: this runner is stopping, so dial again shortly. Beside yamux once the link is up,
    /// the agent keeps the link until its streams finish; in place of a challenge, the hello was
    /// not taken. An agent that does not know it skips it or dials again later.
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
