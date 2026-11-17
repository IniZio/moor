//! One line of a run's history. Rows are only appended.

use super::{RunEnd, RunOutcome, RunPhase, RunState, StepId};

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
    /// No Box was started, because the previous run's Box was still open.
    Skipped,
    /// The start was refused, or no agent could run in the Box.
    Refused {
        /// The phase the run was in.
        phase_before: RunPhase,
        /// Why.
        detail: String,
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
            RunState::Done(RunEnd::Refused { detail }) => Self::Refused {
                phase_before: from,
                detail: detail.clone(),
            },
            RunState::Done(RunEnd::Ended { outcome, .. }) => Self::Ended {
                phase_before: from,
                outcome: outcome.clone(),
            },
        }
    }
}
