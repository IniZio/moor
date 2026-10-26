//! Which of a user's machines a Box gets, or the note saying why it gets none.

use std::collections::BTreeMap;

use super::UserMachine;

/// The machine chosen for a Box, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachinePick {
    /// Attach this machine.
    Attach(UserMachine),
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
            Self::Attach(_) => None,
            Self::LeftOut { reason, .. } => Some(format!("machine left out: {reason}")),
        }
    }
}

/// The first of `own` (the requester's machines, by name) that holds every label in `labels`
/// and no open Box holds; `held` maps a machine name to the Box that holds it.
#[must_use]
pub fn pick_machine(
    labels: &[String],
    own: &[UserMachine],
    held: &BTreeMap<String, String>,
) -> MachinePick {
    let mut fitting = own
        .iter()
        .filter(|m| labels.iter().all(|l| m.labels.contains(l)))
        .peekable();
    let Some(first) = fitting.peek().copied() else {
        return MachinePick::LeftOut {
            labels: labels.to_vec(),
            reason: format!("you have no machine with labels {}", labels.join(", ")),
        };
    };
    if let Some(free) = fitting.find(|m| !held.contains_key(&m.name)) {
        return MachinePick::Attach(free.clone());
    }
    let holder = held.get(&first.name).map_or("", String::as_str);
    MachinePick::LeftOut {
        labels: labels.to_vec(),
        reason: format!("machine {} is attached to Box {holder}", first.name),
    }
}

#[cfg(test)]
mod tests;
