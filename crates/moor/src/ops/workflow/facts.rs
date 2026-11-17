//! What the tick reads of a workflow's runs, and how the decider sees each one.

use moor_core::decide::workflow::{PriorRun, WorkflowRepoFacts};
use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    HistoryEntry, RunEnd, RunId, RunOutcome, RunState, StepId, Workflow, WorkflowRun,
};
use moor_core::BoxId;

use crate::store::{Store, StoreError};

/// How many of a repo's latest unskipped runs the breaker reads; it needs only a streak of 3.
const HISTORY: u64 = 20;

/// What the tick reads of a workflow's runs.
pub(super) struct History {
    /// Each repo's latest run of any kind, which sets its latest due time.
    pub(super) latest: Vec<WorkflowRun>,
    /// Each repo's latest run that was a real attempt, however many skips and refusals followed.
    real: Vec<WorkflowRun>,
    /// Each repo's latest runs that were not skipped, for the breaker.
    pub(super) unskipped: Vec<WorkflowRun>,
}

impl History {
    /// The decider's facts: each repo's latest due time and its latest real run. Skipped and
    /// refused runs are looked through, so a skip does not unblock the next firing.
    pub(super) fn facts(&self, store: &Store) -> Result<Vec<WorkflowRepoFacts>, StoreError> {
        let mut facts = Vec::new();
        for run in &self.latest {
            let prior = match self
                .real
                .iter()
                .find(|r| r.repo == run.repo)
                .and_then(|r| real_run(&r.state))
            {
                Some(Real::Prior(p)) => Some(p),
                Some(Real::TimedOut(id)) => Some(PriorRun::TimedOut {
                    box_open: box_is_open(store, id)?,
                }),
                None => None,
            };
            facts.push(WorkflowRepoFacts {
                repo: run.repo.clone(),
                last_due: Some(run.due_at),
                prior,
            });
        }
        Ok(facts)
    }
}

/// A run that is a real attempt, as the decider sees it.
enum Real {
    /// Settled without looking at a Box.
    Prior(PriorRun),
    /// Timed out; its Box decides.
    TimedOut(BoxId),
}

/// `state` as a real attempt; none for a skipped or refused run. A run that waits counts as
/// started, and every ending but a timeout as finished: only a timeout keeps its Box in the way.
const fn real_run(state: &RunState) -> Option<Real> {
    match state {
        RunState::Queued => Some(Real::Prior(PriorRun::Starting)),
        RunState::Running { .. } | RunState::Waiting { .. } => Some(Real::Prior(PriorRun::Started)),
        RunState::Done(RunEnd::Ended {
            box_id, outcome, ..
        }) => match outcome {
            RunOutcome::TimedOut { .. } => Some(Real::TimedOut(*box_id)),
            RunOutcome::Delivered { .. }
            | RunOutcome::Failed { .. }
            | RunOutcome::Incomplete { .. }
            | RunOutcome::Declined { .. } => Some(Real::Prior(PriorRun::Finished)),
        },
        RunState::Done(RunEnd::SkippedOverlap | RunEnd::Refused { .. }) => None,
    }
}

/// Reads what the tick needs of `w`'s runs, each query independent of how many skips there are.
pub(super) fn history(store: &Store, w: &Workflow) -> Result<History, StoreError> {
    let mut h = History {
        latest: Vec::new(),
        real: Vec::new(),
        unskipped: Vec::new(),
    };
    for repo in &w.repos {
        h.latest.extend(store.recent_workflow_runs(w.id, repo, 1)?);
        h.real.extend(store.latest_real_run(w.id, repo)?);
        h.unskipped
            .extend(store.recent_unskipped_runs(w.id, repo, HISTORY)?);
    }
    Ok(h)
}

/// Whether Box `id` exists and is neither closed nor rejected.
pub(super) fn box_is_open(store: &Store, id: BoxId) -> Result<bool, StoreError> {
    use moor_core::domain::box_state::BoxLifecycle;
    Ok(store
        .box_row(id)?
        .is_some_and(|r| !matches!(r.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected)))
}

/// The history lines of run `run` that is at `at`, and when the agent was last told something. A
/// run from before history kept an `Entered` line reads as entered at its pointer.
pub(super) fn history_of(
    store: &Store,
    run: RunId,
    at: (&StepId, u8),
) -> Result<(Vec<HistoryEntry>, Option<Ts>), StoreError> {
    let entries = store.run_entries(run)?;
    let mut history: Vec<HistoryEntry> = entries.iter().map(|(_, e)| e.clone()).collect();
    if !history
        .iter()
        .any(|e| matches!(e, HistoryEntry::Entered { .. }))
    {
        history.insert(
            0,
            HistoryEntry::Entered {
                step: at.0.clone(),
                try_n: at.1,
            },
        );
    }
    let steered = entries.iter().rev().find_map(|(when, e)| {
        matches!(e, HistoryEntry::Sent { .. } | HistoryEntry::Reminded { .. }).then_some(*when)
    });
    Ok((history, steered))
}
