//! The `node-list` value.

use serde::{Deserialize, Serialize};

use crate::request::WarmStarts;
use crate::user::UserId;

/// Who may use a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeScope {
    /// Every user.
    Shared,
    /// Its owner only.
    Personal,
}

impl NodeScope {
    /// The lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Personal => "personal",
        }
    }
}

/// One node as `node-list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeView {
    /// The node's name.
    pub name: String,
    /// Who may use it.
    pub scope: NodeScope,
    /// The owning user of a personal node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserId>,
    /// The runtime its Boxes run on.
    pub runtime: String,
    /// Whether its agent link is up; a shared node always is.
    pub online: bool,
    /// When an offline node went down, unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_since: Option<i64>,
    /// Boxes holding a slot on it.
    pub used: usize,
    /// Boxes pending on it.
    pub waiting: usize,
    /// Most Boxes it hosts; none means no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_boxes: Option<usize>,
    /// MiB free for another Box by its last memory report, when its host reports memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_free_mib: Option<u64>,
    /// MiB free on its host's disk by its last report, when its host reports disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free_mib: Option<u64>,
    /// Whether its host's free disk is below what it keeps for another Box.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disk_low: bool,
    /// Its host's memory pressure, PSI `some avg10` in hundredths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_psi_centi: Option<u32>,
    /// Seconds since its last memory report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_report_age_secs: Option<i64>,
    /// Since when its host has been under sustained memory pressure, unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_since: Option<i64>,
    /// Whether a Box on it can start from a checkpoint; none on a shared node, or for an agent
    /// that never said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_starts: Option<WarmStarts>,
    /// MiB of its host the Boxes it hosts hold by their last reports; none when no Box reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_memory_mib: Option<u64>,
    /// Its labels.
    pub labels: Vec<String>,
}

/// What starting a Box with no node named would do right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PickOutcome {
    /// The Box starts at once.
    Starts,
    /// The Box waits for room on the node.
    Waits,
    /// The start fails.
    Refused,
}

/// The node a start would pick for the caller, and what happens there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPick {
    /// The node it picks; none when no node can host the Box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// What happens on it.
    pub outcome: PickOutcome,
    /// Why it waits or fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Value of `node-list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeListValue {
    /// The nodes the caller sees, shared first.
    pub nodes: Vec<NodeView>,
    /// When the controller read the nodes, unix seconds.
    #[serde(default)]
    pub at: i64,
    /// What a start naming no node would do for the caller; none when the caller is no one user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<StartPick>,
}

impl NodeView {
    /// `used of max` slots, or `used` alone on a node with no limit.
    #[must_use]
    pub fn slots(&self) -> String {
        self.max_boxes.map_or_else(
            || self.used.to_string(),
            |m| format!("{} of {m}", self.used),
        )
    }

    /// How it stands: online or offline since when, slots, free memory and warm starts.
    #[must_use]
    pub fn status(&self, now: i64) -> String {
        let state = match self.offline_since {
            _ if self.online => "online".to_owned(),
            Some(since) => format!("offline for {}", ago(since, now)),
            None => "offline".to_owned(),
        };
        let mut out = format!("{state}, {} Boxes", self.slots());
        if let Some(free) = self.mem_free_mib {
            out = format!("{out}, {} GiB free", free / 1024);
        }
        match self.warm_starts {
            Some(WarmStarts::On) => out.push_str(", warm starts on"),
            Some(WarmStarts::Off { .. }) => out.push_str(", warm starts off"),
            None => {}
        }
        out
    }

    /// One line: name, scope, runtime and [`Self::status`].
    #[must_use]
    pub fn line(&self, now: i64) -> String {
        let scope = match self.scope {
            NodeScope::Shared => "shared",
            NodeScope::Personal => "yours",
        };
        format!(
            "{} ({scope}, {}) {}",
            self.name,
            self.runtime,
            self.status(now)
        )
    }
}

impl StartPick {
    /// One sentence on what a start does.
    #[must_use]
    pub fn sentence(&self) -> String {
        let node = self.node.as_deref().unwrap_or("no node");
        match (self.outcome, &self.reason) {
            (PickOutcome::Starts, _) => format!("A start with no node named goes to {node}."),
            (PickOutcome::Waits, Some(r)) => {
                format!("A start with no node named waits on {node}: {r}.")
            }
            (PickOutcome::Refused, Some(r)) => format!("A start with no node named fails: {r}."),
            (_, None) => format!(
                "A start with no node named is {:?} on {node}.",
                self.outcome
            ),
        }
    }
}

impl NodeListValue {
    /// The listing as text for a person: one line per node, then what a start would do.
    #[must_use]
    pub fn text(&self) -> String {
        if self.nodes.is_empty() {
            return "No node is available to you. Add your machine with `moor-machine-agent enroll`."
                .to_owned();
        }
        let mut out: Vec<String> = self.nodes.iter().map(|n| n.line(self.at)).collect();
        if let Some(pick) = &self.start {
            out.push(pick.sentence());
        }
        out.join("\n")
    }
}

/// How long ago `since` was at `now`, both unix seconds, in its largest whole unit.
fn ago(since: i64, now: i64) -> String {
    let secs = now.saturating_sub(since).max(0);
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// One provider check that failed, as `provider-ready` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCheck {
    /// The check's name.
    pub check: String,
    /// The check's status.
    pub status: String,
    /// The short error class crabbox reports, such as `config`, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// The adapter's error text as one clean, bounded line, when crabbox reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What crabbox suggests, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Whether one provider's credentials are in place on the controller host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderReady {
    /// The provider, a node runtime.
    pub provider: String,
    /// Whether every check passed.
    pub ok: bool,
    /// The checks that failed.
    pub failing: Vec<ProviderCheck>,
}

/// The `provider-ready` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderReadyValue {
    /// One entry per distinct node runtime, sorted.
    pub providers: Vec<ProviderReady>,
}
