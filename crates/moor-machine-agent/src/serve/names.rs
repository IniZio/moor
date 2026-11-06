//! What an op is called in the agent's status and its logs.

use moor_client::link::ops::Op;

/// The Box id or lease slug `op` names.
pub(super) fn target(op: &Op) -> Option<String> {
    match op {
        Op::Exec { box_id, .. }
        | Op::Hook { box_id, .. }
        | Op::Relay { box_id, .. }
        | Op::Sync { box_id, .. }
        | Op::Pull { box_id, .. }
        | Op::Listen { box_id, .. }
        | Op::Remove { box_id } => Some(box_id.to_string()),
        Op::Lease { slug, .. } => slug.clone(),
        Op::Dial { slug, .. } => Some(slug.clone()),
        Op::Checkpoint { act } => act.target().map(str::to_owned),
        Op::Sweep { .. } | Op::Status => None,
    }
}

/// The op's name on the wire.
pub(super) const fn kind(op: &Op) -> &'static str {
    match op {
        Op::Exec { .. } => "exec",
        Op::Hook { .. } => "hook",
        Op::Relay { .. } => "relay",
        Op::Sync { .. } => "sync",
        Op::Pull { .. } => "pull",
        Op::Listen { .. } => "listen",
        Op::Lease { .. } => "lease",
        Op::Checkpoint { .. } => "checkpoint",
        Op::Dial { .. } => "dial",
        Op::Sweep { .. } => "sweep",
        Op::Remove { .. } => "remove",
        Op::Status => "status",
    }
}
