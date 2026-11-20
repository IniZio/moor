//! What a run is about, for the rule of one open run per workflow and subject, and the cancel a
//! newer head or a close makes of a pull request's open run.
//!
//! A run for a pull request is about that pull request at any head. Every other run, cron or
//! manual without a pull request, is about the workflow itself. Two subjects never block each
//! other, so two pull requests run at once.

use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    CancelCause, RunCause, RunEnd, RunOutcome, RunState, WorkflowId, WorkflowRun,
};
use moor_schema::tables::WorkflowRuns;
use sea_query::{Cond, Expr, ExprTrait as _, Order, Query};

use super::row::{read_run, run_of, RUN_COLS, STARTED, STARTING, WAITING};
use super::runs::{move_in, Open};
use crate::store::db::{Db, DbExt as _, Tx};
use crate::store::{Store, StoreError};

/// What a run is about: none for the workflow's own runs, else the repo and number of its pull
/// request.
pub(super) type Subject<'a> = Option<(&'a ForgeRepo, u64)>;

/// The subject of a run with `cause`.
pub(super) fn subject_of(cause: &RunCause) -> Subject<'_> {
    cause.subject().map(|p| (&p.repo, p.number))
}

/// The condition that a row is about `subject`.
pub(super) fn about(subject: Subject<'_>) -> Cond {
    match subject {
        None => Cond::all().add(Expr::col(WorkflowRuns::PullNumber).is_null()),
        Some((repo, number)) => Cond::all()
            .add(Expr::col(WorkflowRuns::Repo).eq(repo.to_string()))
            .add(Expr::col(WorkflowRuns::PullNumber).eq(i64::try_from(number).unwrap_or(i64::MAX))),
    }
}

/// The run of `workflow` about `subject` that is claimed, working or waiting, read in `tx`.
pub(super) fn open_run(
    tx: &dyn Db,
    workflow: WorkflowId,
    subject: Subject<'_>,
) -> Result<Option<WorkflowRun>, StoreError> {
    tx.rows(
        Query::select()
            .columns(RUN_COLS)
            .from(WorkflowRuns::Table)
            .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
            .and_where(Expr::col(WorkflowRuns::Outcome).is_in([STARTING, STARTED, WAITING]))
            .cond_where(about(subject))
            .order_by(WorkflowRuns::Id, Order::Desc)
            .limit(1),
        |r| read_run(r, 0),
    )?
    .map(|row| run_of(workflow, &row?))
    .next()
    .transpose()
}

/// Ends `open` `Cancelled` with `cause` at `at`, in `tx` numbered `seq`, compare-and-set on the
/// state, step and try it was read at; the rows changed. A run still being started has no Box
/// and no step yet, so it ends [`RunEnd::CancelledUnstarted`], and its start then finds the claim
/// settled and closes the Box it made.
pub(super) fn cancel_in(
    tx: &Tx<'_>,
    seq: i64,
    open: &WorkflowRun,
    cause: CancelCause,
    at: Ts,
) -> Result<usize, StoreError> {
    let (from, box_id, step, try_n) = match &open.state {
        RunState::Running { box_id, at, try_n } => (Open::Running, *box_id, at, *try_n),
        RunState::Waiting {
            box_id, at, try_n, ..
        } => (Open::Waiting, *box_id, at, *try_n),
        RunState::Queued => {
            let to = RunState::Done(RunEnd::CancelledUnstarted { cause });
            return move_in(tx, seq, open.id, (Open::Queued, &to), (None, &[], at, true));
        }
        RunState::Done(_) => return Ok(0),
    };
    let to = RunState::Done(RunEnd::Ended {
        box_id,
        ended_at: at,
        outcome: RunOutcome::Cancelled {
            at: step.clone(),
            cause,
        },
    });
    move_in(
        tx,
        seq,
        open.id,
        (from, &to),
        (Some((step, try_n)), &[], at, true),
    )
}

impl Store {
    /// Cancels the open run of `workflow` for pull request `number` on `repo` with `cause`, at
    /// `at`, whether it is being started, at a step or waiting. The run as it was before the
    /// cancel; none when there was nothing to cancel, so a repeated cancel changes nothing.
    pub(crate) fn cancel_pull_run(
        &self,
        workflow: WorkflowId,
        (repo, number): (&ForgeRepo, u64),
        cause: &CancelCause,
        at: Ts,
    ) -> Result<Option<WorkflowRun>, StoreError> {
        let mut cancelled = None;
        self.numbered(|tx, seq| {
            let Some(open) = open_run(tx, workflow, Some((repo, number)))? else {
                return Ok(0);
            };
            let changed = cancel_in(tx, seq, &open, cause.clone(), at)?;
            if changed == 1 {
                cancelled = Some(open);
            }
            Ok(changed)
        })?;
        Ok(cancelled)
    }
}
