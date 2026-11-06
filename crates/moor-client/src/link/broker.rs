//! The machine runner's links broker: a socket on its host, or a hello on its listener, that
//! reaches the agents linked to the runner.
//!
//! Through it the controller asks a pool host for a lease or a dial into a lease's VM, and any
//! agent for its status.

use serde::{Deserialize, Serialize};

use super::lease;
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
    /// [`Op::Lease`].
    Lease {
        /// What to do.
        verb: lease::Verb,
        /// `moor-<controller>-<id>`; none for a list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slug: Option<String>,
        /// The OpenSSH public key an authorize lets in; none for every other verb.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        /// The cache volumes a warmup mounts; none for every other verb.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache: Option<lease::Cache>,
        /// The machine class a warmup boots; none for every other verb.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
    },
    /// [`Op::Checkpoint`].
    Checkpoint {
        /// What to do.
        act: lease::CheckpointAct,
    },
    /// [`Op::Dial`].
    Dial {
        /// `moor-<controller>-<id>`.
        slug: String,
        /// How long the caller waits for the VM's sshd; the agent's own limit when none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_secs: Option<u64>,
    },
    /// [`Op::Status`].
    Status,
}

impl From<PoolOp> for Op {
    fn from(op: PoolOp) -> Self {
        match op {
            PoolOp::Lease {
                verb,
                slug,
                key,
                cache,
                class,
            } => Self::Lease {
                verb,
                slug,
                key,
                cache,
                class,
            },
            PoolOp::Checkpoint { act } => Self::Checkpoint { act },
            PoolOp::Dial { slug, timeout_secs } => Self::Dial { slug, timeout_secs },
            PoolOp::Status => Self::Status,
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

/// Asks `runner`'s broker to do `verb` to `slug` on machine `machine_id`; a warmup
/// boots with `boot`.
pub async fn lease(
    runner: &Runner,
    machine_id: i64,
    verb: lease::Verb,
    slug: Option<&str>,
    boot: lease::Boot,
) -> Result<lease::Answer, String> {
    let slug = slug.map(str::to_owned);
    leased(
        runner,
        machine_id,
        PoolOp::Lease {
            verb,
            slug,
            key: None,
            cache: boot.cache,
            class: boot.class,
        },
    )
    .await
}

/// Asks `runner`'s broker to let OpenSSH public `key` sign in to `slug`'s VM on machine
/// `machine_id`.
pub async fn authorize(
    runner: &Runner,
    machine_id: i64,
    slug: &str,
    key: &str,
) -> Result<lease::Answer, String> {
    let op = PoolOp::Lease {
        verb: lease::Verb::Authorize,
        slug: Some(slug.to_owned()),
        key: Some(key.to_owned()),
        cache: None,
        class: None,
    };
    leased(runner, machine_id, op).await
}

/// Asks machine `machine_id`, through `runner`'s broker, to act on its checkpoints.
pub async fn checkpoint(
    runner: &Runner,
    machine_id: i64,
    act: lease::CheckpointAct,
) -> Result<lease::Answer, String> {
    leased(runner, machine_id, PoolOp::Checkpoint { act }).await
}

/// Sends lease `op` to machine `machine_id`; its answer.
async fn leased(runner: &Runner, machine_id: i64, op: PoolOp) -> Result<lease::Answer, String> {
    match ask(runner, machine_id, op).await?.1 {
        Reply::Leased(answer) => Ok(answer),
        other => Err(refusal(machine_id, other)),
    }
}

/// Asks `runner`'s broker to dial `slug`'s sshd on machine `machine_id`, finding it within
/// `timeout_secs` when given; the stream to it.
pub async fn dial(
    runner: &Runner,
    machine_id: i64,
    slug: &str,
    timeout_secs: Option<u64>,
) -> Result<Conn, String> {
    let slug = slug.to_owned();
    let op = PoolOp::Dial { slug, timeout_secs };
    match ask(runner, machine_id, op).await? {
        (conn, Reply::Started) => Ok(conn),
        (_, other) => Err(refusal(machine_id, other)),
    }
}

/// Asks `runner`'s broker for machine `machine_id`'s status.
pub async fn status(runner: &Runner, machine_id: i64) -> Result<Status, String> {
    match ask(runner, machine_id, PoolOp::Status).await?.1 {
        Reply::Status(status) => Ok(*status),
        other => Err(refusal(machine_id, other)),
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
