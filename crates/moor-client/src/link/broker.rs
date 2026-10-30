//! The machine runner's links broker: a local socket through which the controller asks a pool
//! host, over the link the runner holds, for a lease or a dial into a lease's VM.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;

use super::lease;
use super::ops::{self, Op, Reply};

/// The broker's socket, beside the runner's socket to the controller.
pub const SOCKET: &str = "links.sock";

/// A client's one request: an op for machine `machine_id`'s agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    /// The machine whose link carries it.
    pub machine_id: i64,
    /// What to ask its agent.
    pub op: PoolOp,
}

/// The ops the broker carries; it takes no other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "op", deny_unknown_fields)]
pub enum PoolOp {
    /// [`Op::Lease`].
    Lease {
        /// What to do.
        verb: lease::Verb,
        /// `moor-<id>`; none for a list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slug: Option<String>,
    },
    /// [`Op::Dial`].
    Dial {
        /// `moor-<id>`.
        slug: String,
    },
}

impl From<PoolOp> for Op {
    fn from(op: PoolOp) -> Self {
        match op {
            PoolOp::Lease { verb, slug } => Self::Lease { verb, slug },
            PoolOp::Dial { slug } => Self::Dial { slug },
        }
    }
}

/// The broker's answer, one line; after a dial the agent started, the VM's bytes follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "broker", deny_unknown_fields)]
pub enum Brokered {
    /// The agent's reply.
    Agent {
        /// As the agent sent it.
        reply: Reply,
    },
    /// The machine holds no link, or its link closed.
    Offline {
        /// The machine asked for.
        machine_id: i64,
        /// Why.
        reason: String,
    },
    /// The broker did not carry the request.
    Refused {
        /// Why.
        reason: String,
    },
}

/// Asks the broker at `socket` to do `verb` to `slug` on machine `machine_id`.
pub async fn lease(
    socket: &Path,
    machine_id: i64,
    verb: lease::Verb,
    slug: Option<&str>,
) -> Result<lease::Answer, String> {
    let slug = slug.map(str::to_owned);
    let (_, reply) = ask(socket, machine_id, PoolOp::Lease { verb, slug }).await?;
    match reply {
        Reply::Leased(answer) => Ok(answer),
        other => Err(refusal(machine_id, other)),
    }
}

/// Asks the broker at `socket` to dial `slug`'s sshd on machine `machine_id`; the stream to it.
pub async fn dial(socket: &Path, machine_id: i64, slug: &str) -> Result<UnixStream, String> {
    let slug = slug.to_owned();
    match ask(socket, machine_id, PoolOp::Dial { slug }).await? {
        (conn, Reply::Started) => Ok(conn),
        (_, other) => Err(refusal(machine_id, other)),
    }
}

/// Sends `op` and reads the agent's reply, with the connection it came on.
async fn ask(socket: &Path, machine_id: i64, op: PoolOp) -> Result<(UnixStream, Reply), String> {
    let mut conn = UnixStream::connect(socket).await.map_err(|e| {
        format!(
            "the links broker at {} is unreachable: {e}",
            socket.display()
        )
    })?;
    ops::write_line(&mut conn, &Ask { machine_id, op })
        .await
        .map_err(|e| format!("the links broker hung up: {e}"))?;
    match ops::read_line(&mut conn).await? {
        Brokered::Agent { reply } => Ok((conn, reply)),
        Brokered::Offline { machine_id, reason } => {
            Err(format!("machine {machine_id} is offline: {reason}"))
        }
        Brokered::Refused { reason } => Err(format!("the links broker refused: {reason}")),
    }
}

/// What machine `machine_id`'s unexpected `reply` says.
fn refusal(machine_id: i64, reply: Reply) -> String {
    match reply {
        Reply::Refused { reason } => format!("machine {machine_id} refused: {reason}"),
        other => format!("machine {machine_id} answered {other:?}"),
    }
}
