//! A Box's gates as `show` reports them.

use crate::gate::{passed_gates, status, wait, GateLine, GateStatus, Gates, Wait};

/// The gates of a Box and where it stands against them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GateView {
    /// The gates' names, in the order they pass.
    pub gates: Vec<String>,
    /// Where the Box stands.
    pub status: GateStatus,
    /// How many gates passed: fewer than `gates` when an `anytime` outcome ended the run first.
    pub gates_passed: usize,
    /// What a person is asked, when the gate waits for one.
    pub wait: Option<Wait>,
    /// What the agent wrote with its newest accepted report that had a note.
    pub note: Option<String>,
    /// How the Box's workflow run ended, such as `cancelled` or `timed_out`, when it ended
    /// short of delivering: the gates no longer decide the Box's state, and nothing moves them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_ended: Option<String>,
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
            gates_passed: passed_gates(gates, lines),
            wait: wait(gates, lines),
            note: lines
                .iter()
                .rev()
                .find_map(GateLine::accepted_note)
                .map(str::to_owned),
            run_ended: None,
        }
    }

    /// This view for a Box whose run ended short of delivering as `ended` names, when it did:
    /// nothing moves its gates any more, so an open or stuck gate waits for nobody.
    #[must_use]
    pub fn ended(mut self, ended: Option<String>) -> Self {
        if ended.is_some() && self.status.can_move() {
            self.wait = None;
        }
        self.run_ended = ended;
        self
    }
}
