//! A Box's gates as `show` reports them.

use crate::gate::{status, wait, GateLine, GateStatus, Gates, Wait};

/// The gates of a Box and where it stands against them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GateView {
    /// The gates' names, in the order they pass.
    pub gates: Vec<String>,
    /// Where the Box stands.
    pub status: GateStatus,
    /// What a person is asked, when the gate waits for one.
    pub wait: Option<Wait>,
    /// What the agent wrote with its newest accepted report that had a note.
    pub note: Option<String>,
}

/// Result of `gate-answer`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GateAnswerValue {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: String,
    /// The gate answered.
    pub gate: String,
    /// What the answer did, in a sentence.
    pub outcome: String,
}

impl GateView {
    /// The view of `gates` after `lines`, oldest first.
    #[must_use]
    pub fn of(gates: &Gates, lines: &[GateLine]) -> Self {
        Self {
            gates: gates.iter().map(|g| g.name.to_string()).collect(),
            status: status(gates, lines),
            wait: wait(gates, lines),
            note: lines
                .iter()
                .rev()
                .find_map(GateLine::accepted_note)
                .map(str::to_owned),
        }
    }
}
