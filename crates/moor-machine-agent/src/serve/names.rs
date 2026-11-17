//! What an op is called in the agent's status and its logs.

use moor_client::link::ops::Op;

/// The Box id `op` names.
pub(super) fn target(op: &Op) -> Option<String> {
    match op {
        Op::Exec { box_id, .. }
        | Op::Hook { box_id, .. }
        | Op::Relay { box_id, .. }
        | Op::Sync { box_id, .. }
        | Op::Pull { box_id, .. }
        | Op::Listen { box_id, .. }
        | Op::Remove { box_id } => Some(box_id.to_string()),
        Op::Sweep { .. } | Op::Status | Op::Boxes => None,
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
        Op::Sweep { .. } => "sweep",
        Op::Remove { .. } => "remove",
        Op::Status => "status",
        Op::Boxes => "boxes",
    }
}
