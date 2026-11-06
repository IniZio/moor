//! Arguments of the machine requests: a user's enrolled machines.

use serde::{Deserialize, Serialize};

use crate::box_key::ControllerId;
use crate::name::BoxId;
use crate::seam::clock::Ts;
use crate::user::{Email, Identity};

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
    /// The crabbox provider the machine offers Boxes on, when it hosts a Box pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
}

/// Arguments of `machine-links`: the runner's whole view of which enrolled machines hold a
/// link, sent again on every change so the stored view converges on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineLinksReq {
    /// The runner that holds these links.
    pub runner: RunnerAddr,
    /// Whether that runner is shutting down; its links read as offline until they move.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub draining: bool,
    /// Every machine the runner has seen since it started; any other is offline.
    pub links: Vec<MachineLink>,
    /// The host memory each Box's VM holds, for the Boxes whose provider says; a Box left out has
    /// none recorded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub box_memory: Vec<BoxHostMemory>,
}

/// Arguments of `machine-box`: the runner reads and writes what it keeps about a Box it serves on
/// a machine, so any runner can wake a Box another prepared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineBoxReq {
    /// What to do with the record.
    pub op: MachineBoxOp,
}

/// What `machine-box` does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum MachineBoxOp {
    /// The record of one Box, if any.
    Get {
        /// The Box.
        box_id: BoxId,
    },
    /// Write a Box's record whole.
    Put(MachineBox),
    /// Remove a Box's record; none is fine.
    Drop {
        /// The Box.
        box_id: BoxId,
    },
    /// Every record on one machine.
    List {
        /// The machine.
        machine_id: i64,
    },
}

/// What the runner keeps about a Box on a machine. The controller stores `reach` and `facts` as
/// the runner wrote them and never reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineBox {
    /// The Box.
    pub box_id: BoxId,
    /// The machine it was set up on.
    pub machine_id: i64,
    /// Its slot there.
    pub slot: u16,
    /// How the runner reaches the machine, as JSON.
    pub reach: String,
    /// What `prepare` printed, as JSON.
    pub facts: String,
    /// How far `prepare` and `suspend` got.
    pub stage: BoxStage,
    /// Whether its files on the machine were removed while it slept; until its next sync.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub evicted: bool,
}

/// How far a Box on a machine got: a sync noted it, `prepare` ran, or `suspend` ran since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum BoxStage {
    /// Synced but never prepared.
    Noted,
    /// Prepared and awake.
    Prepared,
    /// Prepared, then suspended.
    Suspended {
        /// When, in unix seconds.
        at: u64,
    },
}

/// Where the controller reaches the runner that holds a machine's link.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunnerAddr(String);

impl RunnerAddr {
    /// The runner at `addr`.
    #[must_use]
    pub fn new(addr: impl Into<String>) -> Self {
        Self(addr.into())
    }

    /// The address as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The host memory one Box's VM holds on a machine, as its agent last reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxHostMemory {
    /// The machine hosting the Box.
    pub machine_id: i64,
    /// The controller that named the Box's lease; a host shared by controllers lists them all.
    pub controller: ControllerId,
    /// The Box.
    pub box_id: BoxId,
    /// MiB of the host it holds; 0 when its VM is suspended to disk.
    pub host_memory_mib: u64,
    /// MiB of the host its suspended VM needs to resume; none when its provider does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_memory_mib: Option<u64>,
}

/// Whether a machine's agent serves checkpoints, which a warm start needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "state", deny_unknown_fields)]
pub enum WarmStarts {
    /// It does.
    On,
    /// It does not.
    Off {
        /// Why, in words for its owner.
        why: String,
    },
}

/// Whether an enrolled machine's agent holds a link to the runner, since when, and what it
/// speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineLink {
    /// The enrolled machine.
    pub machine_id: i64,
    /// Whether its agent holds a link now.
    pub online: bool,
    /// When the link came up or went down.
    pub since: Ts,
    /// The agent's release, from its last link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The link protocol it spoke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    /// The most Boxes its agent said it will hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_boxes: Option<u32>,
    /// The cache disk, in GB, its agent set aside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_gb: Option<u32>,
    /// What its crabbox provider supports.
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
    /// Whether its agent serves checkpoints; none from an agent that never said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_starts: Option<WarmStarts>,
    /// Its agent's latest live memory report; none from an agent that never sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryReport>,
}

/// A machine's host memory as its agent last reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryReport {
    /// When the runner received it.
    pub at: Ts,
    /// `MemAvailable`, in MiB.
    pub available_mib: u64,
    /// `MemTotal`, in MiB.
    pub total_mib: u64,
    /// PSI `some avg10`, in hundredths; none when the host has no PSI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psi_centi: Option<u32>,
    /// PSI `full avg10`, in hundredths; none when the host has no PSI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psi_full_centi: Option<u32>,
    /// MiB free on the filesystem holding the agent's home; none from an agent that cannot tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free_mib: Option<u64>,
    /// MiB the filesystem holds in all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_total_mib: Option<u64>,
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

/// Arguments of `machine-status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineStatusReq {
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
        /// Its enrolled id, which its agent links in under.
        machine_id: i64,
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
