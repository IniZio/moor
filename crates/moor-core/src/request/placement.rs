//! How far a start that names no node looks for a node.

use serde::{Deserialize, Serialize};

/// How far a start that names no node looks past its first candidate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// Only the first candidate is tried: offline, it refuses. A person starting a Box by hand.
    #[default]
    FirstOnly,
    /// The first online candidate is taken, so a workflow firing is not stopped by a node that
    /// is down.
    FirstOnline,
}

impl Placement {
    /// Whether this is the strict default, for leaving it out of the JSON.
    #[must_use]
    pub const fn is_first_only(&self) -> bool {
        matches!(self, Self::FirstOnly)
    }
}
