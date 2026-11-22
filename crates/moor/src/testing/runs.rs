//! Run states for tests, as the tick records them for a workflow nobody planned.

use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{Plan, RunEnd, RunId, RunOutcome, RunState, WorkflowId};
use moor_core::BoxId;

use crate::store::Store;

/// The id of the run of `workflow` and `repo` due at `due_at`; panics when there is none.
pub(crate) fn run_at(
    store: &Store,
    (workflow, repo, due_at): (WorkflowId, &ForgeRepo, Ts),
) -> RunId {
    store
        .recent_workflow_runs(workflow, 500)
        .unwrap()
        .into_iter()
        .find(|r| &r.repo == repo && r.due_at == due_at)
        .map(|r| r.id)
        .expect("a run of that workflow, repo and due time")
}

/// A run at the one step of the single plan, in Box `box_id`.
pub(crate) fn running(box_id: i64) -> RunState {
    RunState::Running {
        box_id: BoxId::new(box_id),
        at: Plan::single_id(),
        try_n: 1,
    }
}

/// A run whose start was refused for `detail`.
pub(crate) fn refused(detail: &str) -> RunState {
    RunState::Done(RunEnd::Refused {
        detail: detail.to_owned(),
    })
}

/// A run skipped for an open previous Box.
pub(crate) const fn skipped() -> RunState {
    RunState::Done(RunEnd::SkippedOverlap)
}

/// A run of Box `box_id` that ended at `ended_at` as `outcome`.
pub(crate) fn ended(box_id: i64, ended_at: i64, outcome: RunOutcome) -> RunState {
    RunState::Done(RunEnd::Ended {
        box_id: BoxId::new(box_id),
        ended_at: Ts::from_millis(ended_at),
        outcome,
    })
}

/// A run of Box `box_id` that delivered, with no result, at `ended_at`.
pub(crate) fn delivered(box_id: i64, ended_at: i64) -> RunState {
    ended(
        box_id,
        ended_at,
        RunOutcome::Delivered {
            result: String::new(),
            verdict: None,
        },
    )
}

/// A run of Box `box_id` that timed out at `ended_at`.
pub(crate) fn timed_out(box_id: i64, ended_at: i64) -> RunState {
    ended(
        box_id,
        ended_at,
        RunOutcome::TimedOut {
            at: Plan::single_id(),
        },
    )
}
