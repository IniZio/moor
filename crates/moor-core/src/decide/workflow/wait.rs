//! Runs that wait for their owner: how long they waited, and when waiting gives up.

use std::time::Duration;

use crate::name::{BoxId, ForgeRepo};
use crate::seam::clock::Ts;
use crate::workflow::{HistoryEntry, RunState, StepId, WorkflowRun};

/// How long a run waits for an answer before it ends declined.
pub const WAIT_LIMIT: Duration = Duration::from_hours(24 * 7);

/// A run that waited [`WAIT_LIMIT`] for an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unanswered {
    /// The repo the run is for.
    pub repo: ForgeRepo,
    /// The due time the run answers.
    pub due_at: Ts,
    /// The run's Box.
    pub box_id: BoxId,
    /// The approve step it waits at.
    pub at: StepId,
    /// The try of that step.
    pub try_n: u8,
}

/// The runs in `open` that have waited [`WAIT_LIMIT`] since they began to wait.
#[must_use]
pub fn workflow_unanswered(open: &[WorkflowRun], now: Ts) -> Vec<Unanswered> {
    let limit = i64::try_from(WAIT_LIMIT.as_millis()).unwrap_or(i64::MAX);
    open.iter()
        .filter_map(|run| match &run.state {
            RunState::Waiting {
                box_id,
                at,
                try_n,
                since,
            } if now.as_millis().saturating_sub(since.as_millis()) >= limit => Some(Unanswered {
                repo: run.repo.clone(),
                due_at: run.due_at,
                box_id: *box_id,
                at: at.clone(),
                try_n: *try_n,
            }),
            _ => None,
        })
        .collect()
}

/// The milliseconds a run spent in waits that were answered.
///
/// For each `Answered` line this counts the time since the same step was entered. A wait that is
/// still going is not counted here, and the clock of a waiting run is not read at all.
///
/// `lines` are the run's history, each with when it was written.
#[must_use]
pub fn waited_millis(lines: &[(Ts, HistoryEntry)]) -> i64 {
    let mut total: i64 = 0;
    for (i, (at, line)) in lines.iter().enumerate() {
        let HistoryEntry::Answered { step, .. } = line else {
            continue;
        };
        let entered =
            lines
                .get(..i)
                .unwrap_or_default()
                .iter()
                .rev()
                .find_map(|(when, e)| match e {
                    HistoryEntry::Entered { step: s, .. } if s == step => Some(*when),
                    _ => None,
                });
        if let Some(entered) = entered {
            total = total.saturating_add(at.as_millis().saturating_sub(entered.as_millis()));
        }
    }
    total
}
