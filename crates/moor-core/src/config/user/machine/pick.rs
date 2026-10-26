//! Which of a user's machines a Box gets, in which port slot, or the note saying why it gets none.

use std::collections::BTreeMap;

use super::UserMachine;
use crate::config::RepoMachine;

/// How far each slot moves the ports a repo asks for: slot `k` uses port `p + 10·k`.
const SLOT_STRIDE: u32 = 10;

/// The machine chosen for a Box, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachinePick {
    /// Attach this machine in `slot`, forwarding `ports` at the same numbers on both ends.
    Attach {
        /// The machine.
        machine: UserMachine,
        /// The Box's slot on it, `0..capacity`.
        slot: u16,
        /// The repo's ports moved by the slot.
        ports: Vec<u16>,
    },
    /// Start without a machine; `reason` says why.
    LeftOut {
        /// The labels asked for.
        labels: Vec<String>,
        /// Why no machine was attached, for the start note.
        reason: String,
    },
}

impl MachinePick {
    /// The start note for a Box that got no machine.
    #[must_use]
    pub fn note(&self) -> Option<String> {
        match self {
            Self::Attach { .. } => None,
            Self::LeftOut { reason, .. } => Some(format!("machine left out: {reason}")),
        }
    }
}

/// The lowest free slot of the first of `own` (the requester's machines, by name) that holds
/// every label `want` names and has one; `held` maps a machine name to its taken slots and the
/// Box in each.
#[must_use]
pub fn pick_machine(
    want: &RepoMachine,
    own: &[UserMachine],
    held: &BTreeMap<String, BTreeMap<u16, String>>,
) -> MachinePick {
    let left_out = |reason: String| MachinePick::LeftOut {
        labels: want.labels.clone(),
        reason,
    };
    let mut fitting = own
        .iter()
        .filter(|m| want.labels.iter().all(|l| m.labels.contains(l)))
        .peekable();
    let Some(first) = fitting.peek().copied() else {
        return left_out(format!(
            "you have no machine with labels {}",
            want.labels.join(", ")
        ));
    };
    let empty = BTreeMap::new();
    let taken = |m: &UserMachine| held.get(&m.name).unwrap_or(&empty);
    let free = fitting.find_map(|m| {
        let slot = (0..m.capacity).find(|s| !taken(m).contains_key(s))?;
        Some((m, slot))
    });
    let Some((machine, slot)) = free else {
        let boxes: Vec<&str> = taken(first).values().map(String::as_str).collect();
        let noun = if boxes.len() == 1 { "Box" } else { "Boxes" };
        return left_out(format!(
            "machine {} is attached to {noun} {}",
            first.name,
            boxes.join(", ")
        ));
    };
    match slot_ports(&want.ports, slot) {
        Ok(ports) => MachinePick::Attach {
            machine: machine.clone(),
            slot,
            ports,
        },
        Err(port) => left_out(format!(
            "port {port} in slot {slot} of machine {} would be above 65535",
            machine.name
        )),
    }
}

/// `ports` moved to `slot`, or the first port that would pass 65535.
fn slot_ports(ports: &[u16], slot: u16) -> Result<Vec<u16>, u16> {
    let shift = SLOT_STRIDE * u32::from(slot);
    ports
        .iter()
        .map(|&p| u16::try_from(u32::from(p) + shift).map_err(|_| p))
        .collect()
}

#[cfg(test)]
mod tests;
