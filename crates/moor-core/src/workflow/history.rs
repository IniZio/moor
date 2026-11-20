//! One line of a run's history. Rows are only appended.

use super::{Answer, CancelCause, RunEnd, RunOutcome, RunPhase, RunState, StepId, StepReport};
use crate::user::UserId;

/// What happened to a run. The store keeps each line as tagged JSON, so a later kind of line
/// reads as a plain line to anything that does not know it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryEntry {
    /// The firing was claimed and its Box is being started.
    Queued,
    /// The run reached a step; `try_n` counts from 1.
    Entered {
        /// The step.
        step: StepId,
        /// How often the run has entered it.
        try_n: u8,
    },
    /// The step's instructions reached the agent, in a steer.
    Sent {
        /// The step.
        step: StepId,
        /// The try they were sent for.
        try_n: u8,
    },
    /// The agent reported the step and the code gate passed.
    Reported {
        /// The step.
        step: StepId,
        /// The try.
        try_n: u8,
        /// What the agent sent.
        report: StepReport,
    },
    /// The agent reported the step and the code gate refused it.
    GateRefused {
        /// The step.
        step: StepId,
        /// The try.
        try_n: u8,
        /// What was missing, too small or too large.
        missing: Vec<String>,
    },
    /// The agent stopped without the step being done, and was sent the step again. A step gets
    /// one reminder per try.
    Reminded {
        /// The step.
        step: StepId,
        /// The try.
        try_n: u8,
    },
    /// The controller ran a check and it ended with an exit code.
    Checked {
        /// The step.
        step: StepId,
        /// The try.
        try_n: u8,
        /// The exit code; 0 passes. A check stopped at its deadline records -1.
        exit: i32,
        /// The last 4 KiB of the check's output.
        tail: String,
    },
    /// A check's exec stream was cut before it ended. That is no result, so the check runs again
    /// on the next tick.
    CheckLost {
        /// The step.
        step: StepId,
        /// The try.
        try_n: u8,
    },
    /// The owner answered an approve step. The run's wait ends here.
    Answered {
        /// The approve step.
        step: StepId,
        /// Who answered.
        by: UserId,
        /// Yes or no.
        answer: Answer,
        /// What the owner said with it, passed on to the agent.
        note: Option<String>,
    },
    /// An approve step got no answer in time, so the run ends declined.
    Unanswered {
        /// The approve step.
        step: StepId,
    },
    /// The step was entered more often than its `max_tries` and followed its fail edge.
    Exhausted {
        /// The step.
        step: StepId,
    },
    /// No Box was started, because the previous run's Box was still open.
    Skipped,
    /// No Box was started, because the pull request came from a fork whose author lacks write
    /// access to the base repo.
    SkippedFork {
        /// The author's GitHub login.
        author: String,
    },
    /// The start was refused, or no agent could run in the Box.
    Refused {
        /// The phase the run was in.
        phase_before: RunPhase,
        /// Why.
        detail: String,
    },
    /// A newer head or the pull request's close cancelled the run while its Box was being
    /// started.
    CancelledUnstarted {
        /// What ended it.
        cause: CancelCause,
    },
    /// The run ended.
    Ended {
        /// The phase the run was in.
        phase_before: RunPhase,
        /// How it ended.
        outcome: RunOutcome,
    },
}

impl HistoryEntry {
    /// The line for a run that moves from `from` to `to`; for a run recorded already in `to`,
    /// pass [`RunPhase::Queued`] as `from`.
    #[must_use]
    pub fn moving(from: RunPhase, to: &RunState) -> Self {
        match to {
            RunState::Queued => Self::Queued,
            RunState::Running { at, try_n, .. } | RunState::Waiting { at, try_n, .. } => {
                Self::Entered {
                    step: at.clone(),
                    try_n: *try_n,
                }
            }
            RunState::Done(RunEnd::SkippedOverlap) => Self::Skipped,
            RunState::Done(RunEnd::SkippedFork { author }) => Self::SkippedFork {
                author: author.clone(),
            },
            RunState::Done(RunEnd::Refused { detail }) => Self::Refused {
                phase_before: from,
                detail: detail.clone(),
            },
            RunState::Done(RunEnd::CancelledUnstarted { cause }) => Self::CancelledUnstarted {
                cause: cause.clone(),
            },
            RunState::Done(RunEnd::Ended { outcome, .. }) => Self::Ended {
                phase_before: from,
                outcome: outcome.clone(),
            },
        }
    }
}
