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
}

/// What a new Box asks of a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSpec {
    /// Labels the node must carry.
    pub labels: Vec<String>,
    /// The node the user named, if any.
    pub node: Option<NodeName>,
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

/// Picks the node for a new Box. Only the first node in order is tried: an offline or full
/// node never sends the Box to another one. `now` is unix seconds.
#[must_use]
pub fn schedule(
    nodes: &[Node],
    requester: UserId,
    default: &NodeName,
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

    let has_all = |n: &&Node| spec.labels.iter().all(|l| n.labels.contains(l));
    let fits: Vec<&Node> = pool.iter().copied().filter(has_all).collect();
    if fits.is_empty() {
        let have: Vec<String> = ordered(&pool, requester, None)
            .iter()
            .map(|n| format!("{} has {}", n.name, n.labels.join(", ")))
            .collect();
        return fail(if spec.labels.is_empty() {
            format!("no node can host this Box: shared default `{default}` does not exist")
        } else {
            format!(
                "no node has labels {}: {}",
                spec.labels.join(", "),
                have.join("; ")
            )
        });
    }

    let order = if spec.node.is_some() {
        fits
    } else {
        ordered(&fits, requester, Some(default))
    };
    order.first().map_or_else(
        || {
            fail(format!(
                "no node can host this Box: shared default `{default}` does not exist"
            ))
        },
        |first| place(first, default, now),
    )
}

/// Personal nodes by enrollment then machine id (all of them, first wins), then the shared ones
/// with the default first.
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
    if default.is_some() {
        shared.truncate(1);
        shared.retain(|n| default == Some(&n.name));
    }
    mine.extend(shared);
    mine
}

/// Judges the first node in order: offline fails, full waits, otherwise binds.
fn place(n: &Node, default: &NodeName, now: i64) -> Verdict {
    if let Online::Offline { since } = n.online {
        let at = ago(since, now);
        return fail(if &n.name == default {
            format!("{} is offline for {at}: wait for it to come back", n.name)
        } else {
            format!(
                "{} is offline for {at}: start moor-machine-agent on it, or start with --node {default}",
                n.name
            )
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
        _ => Verdict::Bind {
            node: n.name.clone(),
            runtime: n.runtime.clone(),
            machine_id: match n.scope {
                Scope::Personal { machine_id, .. } => Some(machine_id),
                Scope::Shared => None,
            },
        },
    }
}

#[cfg(test)]
mod tests;
