//! The lines a Box's gates are read from: appended in order, never changed.

use crate::workflow::{Answer, StepId, StepReport, StepResult};

/// One thing that happened at a gate.
///
/// A line carries the number of the try it belongs to, from 1 within the span since the gate's
/// last [`GateLine::Reopened`], and [`super::status()`] ignores a line of a try that is over, so
/// replaying a line twice changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GateLine {
    /// The agent reported, and the evidence it listed is in. A refused report is [`Self::Refused`].
    Submitted {
        /// The gate.
        gate: StepId,
        /// The try.
        try_n: u8,
        /// What the agent sent.
        report: StepReport,
    },
    /// The agent reported, and evidence it listed is missing.
    Refused {
        /// The gate.
        gate: StepId,
        /// The try.
        try_n: u8,
        /// The evidence names that were not found.
        missing: Vec<String>,
    },
    /// A check ran to an exit code.
    Checked {
        /// The gate.
        gate: StepId,
        /// The try.
        try_n: u8,
        /// The check's place among the gate's checks, from 0.
        check: u8,
        /// Its exit code.
        exit: i32,
        /// The end of its output.
        tail: String,
    },
    /// A check's output stream was lost before it had an exit code.
    CheckLost {
        /// The gate.
        gate: StepId,
        /// The try.
        try_n: u8,
        /// The check's place among the gate's checks, from 0.
        check: u8,
    },
    /// The agent was quiet with nothing submitted, and was reminded.
    Reminded {
        /// The gate.
        gate: StepId,
        /// The try.
        try_n: u8,
    },
    /// The agent was quiet again after the reminder in the same try.
    Quiet {
        /// The gate.
        gate: StepId,
        /// The try.
        try_n: u8,
    },
    /// The owner answered the gate's question.
    Answered {
        /// The gate.
        gate: StepId,
        /// The try the owner was asked about.
        try_n: u8,
        /// Who answered.
        by: String,
        /// Yes or no.
        answer: Answer,
        /// What they added.
        note: String,
    },
    /// The try failed for a reason the other lines do not give.
    TryFailed {
        /// The gate.
        gate: StepId,
        /// The try.
        try_n: u8,
        /// Why.
        why: String,
    },
    /// A person's steer reopened a stuck gate with fresh tries.
    Reopened {
        /// The gate.
        gate: StepId,
        /// Who.
        by: String,
    },
}

impl GateLine {
    /// What the agent wrote with its report, when this line is an accepted report that says
    /// something.
    #[must_use]
    pub fn accepted_note(&self) -> Option<&str> {
        match self {
            Self::Submitted { report, .. }
                if report.result == StepResult::Ok && !report.note.trim().is_empty() =>
            {
                Some(&report.note)
            }
            _ => None,
        }
    }

    /// The gate the line is about.
    #[must_use]
    pub const fn gate(&self) -> &StepId {
        match self {
            Self::Submitted { gate, .. }
            | Self::Refused { gate, .. }
            | Self::Checked { gate, .. }
            | Self::CheckLost { gate, .. }
            | Self::Reminded { gate, .. }
            | Self::Quiet { gate, .. }
            | Self::Answered { gate, .. }
            | Self::TryFailed { gate, .. }
            | Self::Reopened { gate, .. } => gate,
        }
    }
}
