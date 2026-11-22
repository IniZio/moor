//! The lines a Box's gates are read from: appended in order, never changed.

use super::OutcomeName;
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
    /// moor got no answer from a check, of the gate in hand or of an outcome: it did not run,
    /// or its output stream was lost. Not the agent's doing, so it spends no try.
    NoAnswer {
        /// The gate in hand, or the closing stage.
        gate: StepId,
        /// The try.
        try_n: u8,
        /// Which check.
        check: CheckOf,
        /// Why there was no answer.
        why: Unanswered,
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
    /// moor ran an outcome's checks and they all passed, which ends the run in it: at the
    /// closing stage, or at the gate in hand for an `anytime` outcome.
    Held {
        /// The gate in hand, or the closing stage.
        gate: StepId,
        /// The try.
        try_n: u8,
        /// The outcome.
        outcome: OutcomeName,
    },
    /// moor ran the outcomes' checks at the closing stage and none held, which fails the try.
    NoneHeld {
        /// The closing stage.
        gate: StepId,
        /// The try.
        try_n: u8,
        /// Each outcome tried, with the check that stopped it.
        tried: Vec<Tried>,
    },
}

/// One outcome that did not hold, and the check that stopped it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tried {
    /// The outcome.
    pub outcome: OutcomeName,
    /// The check's place among the outcome's checks, from 0.
    pub check: u8,
    /// Its exit code.
    pub exit: i32,
    /// The end of its output.
    pub tail: String,
}

/// Which check a [`GateLine::NoAnswer`] is about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOf {
    /// A check of the gate in hand, by its place from 0.
    Gate {
        /// The check's place among the gate's checks, from 0.
        check: u8,
    },
    /// A check of an outcome, by its place from 0.
    Outcome {
        /// The outcome.
        outcome: OutcomeName,
        /// The check's place among the outcome's checks, from 0.
        check: u8,
    },
}

/// Why moor got no answer from a check.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Unanswered {
    /// The exec failed before the check ran, such as a busy Box.
    NotRun {
        /// The exec's error.
        reason: String,
    },
    /// The link to the machine was cut before the check had an exit code.
    Lost,
}

/// The check moor last got no answer from, when a stage waits for a person because moor got no
/// answer from its checks twice in a row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NoAnswerCause {
    /// Which check.
    pub check: CheckOf,
    /// Its command as written; none when the gates do not have it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// What it says must be true, when its author wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub says: Option<String>,
    /// Why there was no answer.
    pub reason: Unanswered,
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
            | Self::NoAnswer { gate, .. }
            | Self::Reminded { gate, .. }
            | Self::Quiet { gate, .. }
            | Self::Answered { gate, .. }
            | Self::TryFailed { gate, .. }
            | Self::Reopened { gate, .. }
            | Self::Held { gate, .. }
            | Self::NoneHeld { gate, .. } => gate,
        }
    }

    /// The try the line belongs to; none for a reopening.
    #[must_use]
    pub const fn try_n(&self) -> Option<u8> {
        match self {
            Self::Submitted { try_n, .. }
            | Self::Refused { try_n, .. }
            | Self::Checked { try_n, .. }
            | Self::NoAnswer { try_n, .. }
            | Self::Reminded { try_n, .. }
            | Self::Quiet { try_n, .. }
            | Self::Answered { try_n, .. }
            | Self::TryFailed { try_n, .. }
            | Self::Held { try_n, .. }
            | Self::NoneHeld { try_n, .. } => Some(*try_n),
            Self::Reopened { .. } => None,
        }
    }
}
