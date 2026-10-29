//! Arguments of the machine requests: a user's own machines, reached over the host's ssh.

use serde::{Deserialize, Serialize};

use crate::name::BoxId;
use crate::seam::clock::Ts;
use crate::user::{Email, Identity};

/// Arguments of `machine-add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineAddReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The machine's name.
    pub name: String,
    /// The ssh target, `user@host` or `user@host:port`.
    pub ssh: String,
    /// The machine's labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// How many Boxes may hold it at once, each in its own port slot.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub capacity: u16,
}

/// Arguments of `machine-enroll`: a machine agent adds or replaces its machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineEnrollReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The email the front verified for that identity; only a front socket may send one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<Email>,
    /// The machine's name.
    pub name: String,
    /// The agent's ed25519 public key as an OpenSSH line.
    pub pubkey: String,
    /// The machine's labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// How many Boxes may hold it at once.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub capacity: u16,
}

/// Arguments of `machine-links`: the runner's whole view of which enrolled machines hold a
/// link, sent again on every change so the stored view converges on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineLinksReq {
    /// Every machine the runner has seen since it started; any other is offline.
    pub links: Vec<MachineLink>,
}

/// Whether an enrolled machine's agent holds a link to the runner, and since when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineLink {
    /// The enrolled machine.
    pub machine_id: i64,
    /// Whether its agent holds a link now.
    pub online: bool,
    /// When the link came up or went down.
    pub since: Ts,
}

/// The capacity of a machine added without one.
const fn one() -> u16 {
    1
}

/// True for the default capacity, which the wire leaves out.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
const fn is_one(n: &u16) -> bool {
    *n == 1
}

/// Arguments of `machine-remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineRemoveReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The machine's name.
    pub name: String,
}

/// Arguments of `machine-hold`: a Box claims one of its user's machines or lets go of it. The
/// machine runner sends it over the operator socket for the Box its Proxy named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineHoldReq {
    /// The Box's store id.
    pub id: BoxId,
    /// What the Box asks of its claim.
    pub op: HoldOp,
}

/// What a Box asks of its claim on a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum HoldOp {
    /// Hold a machine, waiting for a slot until `wait_until` or refusing at once without it.
    Claim {
        /// Labels the machine must all hold; the repo's when empty and `machine` is `None`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        labels: Vec<String>,
        /// The one machine wanted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
        /// Box ports forwarded; the repo's when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ports: Option<Vec<u16>>,
        /// How long to wait for a slot.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_until: Option<Ts>,
    },
    /// Let go of the machine held, waited for or slept on.
    Release,
}

impl HoldOp {
    /// A claim on a machine the repo's `machine:` section fits, with its ports, refused at once
    /// when no slot is free.
    #[must_use]
    pub const fn repo() -> Self {
        Self::Claim {
            labels: Vec::new(),
            machine: None,
            ports: None,
            wait_until: None,
        }
    }
}

/// The answer to a `machine-hold`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum HoldValue {
    /// The Box holds `slot` on `machine`.
    Held {
        /// The machine's name.
        machine: String,
        /// Its ssh target.
        ssh: String,
        /// The Box's slot on it.
        slot: u16,
        /// The forwarded ports, moved by the slot.
        ports: Vec<u16>,
    },
    /// The want is recorded; `position` counts from 1 among the user's waiters.
    Waiting {
        /// The want's place in line.
        position: u16,
        /// Why there is no slot yet.
        reason: String,
    },
    /// Nothing was recorded.
    Refused {
        /// Why.
        reason: String,
    },
    /// The claim is gone.
    Released {
        /// The machine held or slept on, if any.
        machine: Option<String>,
    },
}
