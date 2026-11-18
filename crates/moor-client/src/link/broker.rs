//! The machine runner's links broker: a socket on its host, or a hello on its listener, that
//! reaches the agents linked to the runner.
//!
//! Through it the controller asks any agent for its status and opens the stream that carries its
//! Box requests.

use serde::{Deserialize, Serialize};

use super::ops::{self, Op, Reply};
use super::reach::{Conn, Runner, Service};
use super::status::Status;

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
    /// [`Op::Boxes`]: the runner writes that op itself, answers [`Brokered::Spliced`], and
    /// carries the stream's bytes both ways without reading them. The client then speaks the
    /// Box protocol (see [`super::boxes`]) straight to the machine's agent.
    Boxes,
    /// [`Op::Status`].
    Status,
    /// One [`Op::Exec`] or [`Op::Hook`], for a runner that holds no link to the machine; the
    /// runner refuses any other op.
    Run {
        /// The op the agent runs.
        run: Op,
    },
}

impl From<PoolOp> for Op {
    fn from(op: PoolOp) -> Self {
        match op {
            PoolOp::Boxes => Self::Boxes,
            PoolOp::Status => Self::Status,
            PoolOp::Run { run } => run,
        }
    }
}

/// The broker's answer, one line; after a dial the agent started, the VM's bytes follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "broker")]
pub enum Brokered {
    /// The agent's reply.
    Agent {
        /// As the agent sent it.
        reply: Reply,
    },
    /// The stream to the machine's agent is open and carries the Box protocol from here on; its
    /// bytes follow this line.
    Spliced,
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

/// Asks `runner`'s broker for machine `machine_id`'s status.
pub async fn status(runner: &Runner, machine_id: i64) -> Result<Status, String> {
    match ask(runner, machine_id, PoolOp::Status).await?.1 {
        Reply::Status(status) => Ok(*status),
        other => Err(refusal(machine_id, other)),
    }
}

/// Asks `runner`'s broker to run `op`, an exec or a hook, on machine `machine_id`; the agent's
/// reply.
pub async fn run(runner: &Runner, machine_id: i64, op: Op) -> Result<Reply, String> {
    Ok(ask(runner, machine_id, PoolOp::Run { run: op }).await?.1)
}

/// Asks `runner`'s broker for a stream to machine `machine_id`'s agent that carries one Box
/// request ([`super::boxes`]); the caller writes the request line next.
pub async fn boxes(runner: &Runner, machine_id: i64) -> Result<Conn, String> {
    let mut conn = runner.connect(Service::Links).await?;
    let ask = Ask {
        machine_id,
        op: PoolOp::Boxes,
    };
    ops::write_line(&mut conn, &ask)
        .await
        .map_err(|e| format!("the links broker hung up: {e}"))?;
    match ops::read_line(&mut conn).await? {
        Brokered::Spliced => Ok(conn),
        Brokered::Agent { reply } => Err(refusal(machine_id, reply)),
        Brokered::Offline { machine_id, reason } => {
            Err(format!("machine {machine_id} is offline: {reason}"))
        }
        Brokered::Refused { reason } => Err(format!("the links broker refused: {reason}")),
    }
}

/// Sends `op` and reads the agent's reply, with the connection it came on.
async fn ask(runner: &Runner, machine_id: i64, op: PoolOp) -> Result<(Conn, Reply), String> {
    let mut conn = runner.connect(Service::Links).await?;
    ops::write_line(&mut conn, &Ask { machine_id, op })
        .await
        .map_err(|e| format!("the links broker hung up: {e}"))?;
    match ops::read_line(&mut conn).await? {
        Brokered::Agent { reply } => Ok((conn, reply)),
        Brokered::Spliced => {
            Err("the links broker spliced a stream that was not asked for".to_owned())
        }
        Brokered::Offline { machine_id, reason } => {
            Err(format!("machine {machine_id} is offline: {reason}"))
        }
        Brokered::Refused { reason } => Err(format!("the links broker refused: {reason}")),
    }
}

/// Whether a broker call failed because its machine or runner could not be reached or the stream
/// was cut, so the agent may have gone on; a machine that answered said something else.
#[must_use]
pub fn is_link_loss(reason: &str) -> bool {
    const LOST: [&str; 5] = [
        "is not linked",
        "is offline:",
        "is unreachable:",
        "the stream ended",
        "the links broker hung up",
    ];
    !reason.contains(" refused: ") && LOST.iter().any(|word| reason.contains(word))
}

/// What machine `machine_id`'s unexpected `reply` says.
fn refusal(machine_id: i64, reply: Reply) -> String {
    match reply {
        Reply::Refused { reason } => format!("machine {machine_id} refused: {reason}"),
        other => format!("machine {machine_id} answered {other:?}"),
    }
}
