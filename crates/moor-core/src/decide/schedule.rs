//! The node scheduler: which node hosts a new Box, or why none can.

use crate::name::NodeName;
use crate::user::UserId;

/// Who may use a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every user may use it.
    Shared,
    /// Only its owner may use it.
    Personal {
        /// The owning user.
        owner: UserId,
        /// The enrolled machine behind the node.
        machine_id: i64,
        /// Enrollment time, unix seconds.
        enrolled_at: i64,
    },
}

/// Whether the node's agent link is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Online {
    /// The link is up.
    Online,
    /// The link went down at `since`, unix seconds.
    Offline {
        /// When it went down.
        since: i64,
    },
}

/// A node as the store read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The node's name.
    pub name: NodeName,
    /// Who may use it.
    pub scope: Scope,
    /// The runtime its Boxes run on.
    pub runtime: String,
    /// Labels a Box may require.
    pub labels: Vec<String>,
    /// Most Boxes it hosts; `None` means no limit.
    pub max_boxes: Option<usize>,
    /// Whether its link is up.
    pub online: Online,
    /// Boxes holding a slot now.
    pub used: usize,
    /// Boxes already pending on it.
    pub waiting: usize,
    /// Its live memory; `None` when its agent has never reported it, so only the count limits it.
    pub memory: Option<Memory>,
}

/// A node's latest live memory report and the Boxes booting on it, in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Memory {
    /// The host's total memory.
    pub total_mib: u64,
    /// What stays with the host; `None` means [`default_reserve_mib`].
    pub reserve_mib: Option<u64>,
    /// The host's `MemAvailable` when it last reported.
    pub available_mib: u64,
    /// The host's memory pressure, PSI `some avg10` in hundredths; `None` when it has no PSI.
    pub psi_centi: Option<u32>,
    /// When it last reported, unix seconds.
    pub reported_at: i64,
    /// Boxes Starting on it.
    pub starting: usize,
}

/// What one more Box must find free to boot.
pub const BOOT_FLOOR_MIB: u64 = 2048;
/// How old a memory report may be before the node starts nothing more.
pub const REPORT_FRESH_SECS: i64 = 60;
/// PSI `some avg10`, in hundredths, at and above which the node starts nothing more.
pub const PSI_LIMIT_CENTI: u32 = 1000;

/// The host's share when none is configured: 4 GiB plus a tenth of `total_mib`.
#[must_use]
pub const fn default_reserve_mib(total_mib: u64) -> u64 {
    4096 + total_mib / 10
}

/// Why a node has no room for one more Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortfall {
    /// Its last memory report is too old.
    Stale,
    /// Its host is under memory pressure.
    Pressure,
    /// Too little is free.
    Low {
        /// What is free for another Box.
        free_mib: u64,
    },
}

impl Shortfall {
    /// Why a Box waits for `node`, for the user.
    #[must_use]
    pub fn reason(&self, node: &NodeName) -> String {
        match self {
            Self::Stale => format!("no recent memory report from {node}"),
            Self::Pressure => format!("memory pressure on {node}"),
            Self::Low { free_mib } => format!(
                "waiting for capacity on {node}: {} free, needs {}",
                gib(*free_mib),
                gib(BOOT_FLOOR_MIB)
            ),
        }
    }
}

impl Memory {
    /// What stays with the host.
    #[must_use]
    pub const fn reserve(&self) -> u64 {
        match self.reserve_mib {
            Some(r) => r,
            None => default_reserve_mib(self.total_mib),
        }
    }

    /// What is free for another Box: available less the reserve and a boot floor per Box
    /// Starting, since a booting guest has not shown up in the report yet.
    #[must_use]
    pub const fn free_mib(&self) -> u64 {
        let booting = BOOT_FLOOR_MIB.saturating_mul(self.starting as u64);
        self.available_mib
            .saturating_sub(self.reserve())
            .saturating_sub(booting)
    }

    /// Why one more Box does not fit at `now`, unix seconds; `None` when it does.
    #[must_use]
    pub fn shortfall(&self, now: i64) -> Option<Shortfall> {
        if now.saturating_sub(self.reported_at) > REPORT_FRESH_SECS {
            return Some(Shortfall::Stale);
        }
        if self.psi_centi.is_some_and(|p| p >= PSI_LIMIT_CENTI) {
            return Some(Shortfall::Pressure);
        }
        let free_mib = self.free_mib();
        (free_mib < BOOT_FLOOR_MIB).then_some(Shortfall::Low { free_mib })
    }
}

/// `mib` as GiB for a message: whole when it divides evenly, else one decimal.
#[must_use]
pub fn gib(mib: u64) -> String {
    if mib.is_multiple_of(1024) {
        format!("{} GiB", mib / 1024)
    } else {
        #[expect(clippy::cast_precision_loss, reason = "a message rounds it anyway")]
        let g = mib as f64 / 1024.0;
        format!("{g:.1} GiB")
    }
}

/// What a new Box asks of a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSpec {
    /// Labels the node must carry.
    pub labels: Vec<String>,
    /// The node the user named, if any.
    pub node: Option<NodeName>,
    /// The guest memory it is given, in MiB.
    pub mem_mib: u64,
}

/// The scheduler's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Start the Box on this node now.
    Bind {
        /// The chosen node.
        node: NodeName,
        /// Its runtime.
        runtime: String,
        /// The machine behind a personal node.
        machine_id: Option<i64>,
    },
    /// The node is full; the Box waits for it.
    Pending {
        /// The node it waits for.
        node: NodeName,
        /// Why, for the user.
        reason: String,
    },
    /// No node can host it.
    Fail {
        /// Why, for the user.
        reason: String,
    },
}

/// A failing verdict.
const fn fail(reason: String) -> Verdict {
    Verdict::Fail { reason }
}

/// Whether `n` is a personal node of `who`.
fn owned_by(n: &Node, who: UserId) -> bool {
    matches!(n.scope, Scope::Personal { owner, .. } if owner == who)
}

/// Node names for a message, or `none`.
fn names(nodes: &[&Node]) -> String {
    if nodes.is_empty() {
        return "none".to_owned();
    }
    let list: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
    list.join(", ")
}

/// How long ago `since` was at `now`, both unix seconds, in its largest whole unit: `12m`, `3h`, `2d`.
fn ago(since: i64, now: i64) -> String {
    let secs = now.saturating_sub(since).max(0);
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// The failure for a name that is not among the requester's visible nodes.
fn unknown(want: &NodeName, visible: &[&Node], requester: UserId) -> Verdict {
    let yours: Vec<&Node> = visible
        .iter()
        .copied()
        .filter(|n| owned_by(n, requester))
        .collect();
    let shared: Vec<&Node> = visible
        .iter()
        .copied()
        .filter(|n| n.scope == Scope::Shared)
        .collect();
    fail(format!(
        "no node named {want}: yours are {}; shared: {}",
        names(&yours),
        names(&shared)
    ))
}

/// The failure when no node is a candidate at all.
fn nowhere(default: Option<&NodeName>) -> Verdict {
    fail(default.map_or_else(
        || {
            "no node can host this Box: enroll a machine with moor-machine-agent enroll \
             --runtime <provider>"
                .to_owned()
        },
        |d| format!("no node can host this Box: shared default `{d}` does not exist"),
    ))
}

/// Picks the node for a new Box.
///
/// Only the first node in order is tried: an offline or full node never sends the Box to
/// another one. With no `default`, only personal nodes are candidates. `now` is unix seconds.
#[must_use]
pub fn schedule(
    nodes: &[Node],
    requester: UserId,
    default: Option<&NodeName>,
    spec: &BoxSpec,
    now: i64,
) -> Verdict {
    let visible: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.scope == Scope::Shared || owned_by(n, requester))
        .collect();
    let pool: Vec<&Node> = match &spec.node {
        Some(want) => {
            let named: Vec<&Node> = visible
                .iter()
                .copied()
                .filter(|n| &n.name == want)
                .collect();
            if named.is_empty() {
                return unknown(want, &visible, requester);
            }
            named
        }
        None => visible,
    };
    if pool.is_empty() {
        return nowhere(default);
    }

    let has_all = |n: &&Node| spec.labels.iter().all(|l| n.labels.contains(l));
    let fits: Vec<&Node> = pool.iter().copied().filter(has_all).collect();
    if fits.is_empty() {
        let have: Vec<String> = ordered(&pool, requester, None)
            .iter()
            .map(|n| format!("{} has {}", n.name, n.labels.join(", ")))
            .collect();
        return fail(format!(
            "no node has labels {}: {}",
            spec.labels.join(", "),
            have.join("; ")
        ));
    }

    let order: Vec<&Node> = if spec.node.is_some() {
        fits
    } else {
        ordered(&fits, requester, default)
            .into_iter()
            .filter(|n| n.scope != Scope::Shared || Some(&n.name) == default)
            .collect()
    };
    order
        .first()
        .map_or_else(|| nowhere(default), |first| place(first, default, now))
}

/// Personal nodes by enrollment then machine id, then the shared ones with the default first.
fn ordered<'a>(nodes: &[&'a Node], requester: UserId, default: Option<&NodeName>) -> Vec<&'a Node> {
    let mut mine: Vec<&Node> = nodes
        .iter()
        .copied()
        .filter(|n| owned_by(n, requester))
        .collect();
    mine.sort_by_key(|n| match n.scope {
        Scope::Personal {
            machine_id,
            enrolled_at,
            ..
        } => (enrolled_at, machine_id),
        Scope::Shared => (i64::MAX, i64::MAX),
    });
    let mut shared: Vec<&Node> = nodes
        .iter()
        .copied()
        .filter(|n| n.scope == Scope::Shared)
        .collect();
    shared.sort_by_key(|n| default != Some(&n.name));
    mine.extend(shared);
    mine
}

/// Judges the first node in order: offline fails, full by count or memory waits, otherwise binds.
fn place(n: &Node, default: Option<&NodeName>, now: i64) -> Verdict {
    if let Online::Offline { since } = n.online {
        let at = ago(since, now);
        return fail(match default {
            Some(d) if d == &n.name => {
                format!("{} is offline for {at}: wait for it to come back", n.name)
            }
            Some(d) => format!(
                "{} is offline for {at}: start moor-machine-agent on it, or start with --node {d}",
                n.name
            ),
            None => format!(
                "{} is offline for {at}: start moor-machine-agent on it",
                n.name
            ),
        });
    }
    match n.max_boxes {
        Some(max) if n.used >= max => Verdict::Pending {
            node: n.name.clone(),
            reason: format!(
                "{} is full ({} of {max} Boxes); {} Boxes ahead",
                n.name, n.used, n.waiting
            ),
        },
        _ => n.memory.and_then(|m| m.shortfall(now)).map_or_else(
            || bind(n),
            |short| Verdict::Pending {
                node: n.name.clone(),
                reason: short.reason(&n.name),
            },
        ),
    }
}

/// Binds the Box to `n`.
fn bind(n: &Node) -> Verdict {
    Verdict::Bind {
        node: n.name.clone(),
        runtime: n.runtime.clone(),
        machine_id: match n.scope {
            Scope::Personal { machine_id, .. } => Some(machine_id),
            Scope::Shared => None,
        },
    }
}

#[cfg(test)]
mod tests;
