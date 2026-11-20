//! Recording a firing: a cron one the tick claims, and a manual one a person asks for.
//!
//! Every write is one transaction in [`Store::numbered`], and those are serialized on the change
//! counter, so a read in the transaction cannot be overtaken by another claim.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::workflow::{
    HistoryEntry, NewRun, Plan, RunEnd, RunOutcome, RunPhase, RunState, WorkflowId, WorkflowRun,
};
use moor_core::BoxId;
use moor_schema::tables::{BoxState, WorkflowRuns};
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query};

use super::history::append_history;
use super::row::{read_run, run_of, state_cols, subject_cols, STARTED, STARTING, WAITING};
use super::runs::real_run;
use crate::store::db::{Db, DbExt as _, OptionalExtension as _};
use crate::store::{Store, StoreError};

/// What asking for a manual run came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualClaim {
    /// The run is recorded.
    Recorded,
    /// A run of the workflow is open, so nothing was recorded. It is that run.
    Open(Box<WorkflowRun>),
}

impl Store {
    /// Records one firing. False, with nothing changed, when a unique key already holds it: a cron
    /// firing on its workflow, repo and due time, a pull request run on its pull request and head.
    /// A firing repeated after a crash then records once. The run's history opens with the line
    /// for its state.
    pub(crate) fn record_workflow_run(&self, run: &NewRun) -> Result<bool, StoreError> {
        self.numbered(|tx, seq| insert(tx, seq, run))
    }

    /// Records a run its owner asked for, unless the workflow has a run open, which is looked for
    /// in the transaction that inserts the row. A read before the write would race the tick, which
    /// could claim a cron run with another due time in between. No unique key covers a manual
    /// run, so the open run is the only thing that stops it.
    pub(crate) fn claim_manual_run(&self, run: &NewRun) -> Result<ManualClaim, StoreError> {
        let mut open = None;
        self.numbered(|tx, seq| {
            open = blocking(tx, run.workflow)?;
            if open.is_some() {
                return Ok(0);
            }
            insert(tx, seq, run)
        })?;
        Ok(open.map_or(ManualClaim::Recorded, |run| {
            ManualClaim::Open(Box::new(run))
        }))
    }
}

/// Inserts `run` and the first line of its history; the rows inserted, none when a unique key
/// already holds it.
fn insert(tx: &dyn Db, seq: i64, run: &NewRun) -> Result<usize, StoreError> {
    let cols = state_cols(&run.state);
    let plan = run.plan.as_ref().map(Plan::to_json).transpose()?;
    let (pull_number, head_sha) = subject_cols(&run.cause);
    let id = tx
        .rows(
            Query::insert()
                .into_table(WorkflowRuns::Table)
                .columns([
                    WorkflowRuns::Workflow,
                    WorkflowRuns::Repo,
                    WorkflowRuns::DueAt,
                    WorkflowRuns::Box,
                    WorkflowRuns::Outcome,
                    WorkflowRuns::Detail,
                    WorkflowRuns::EndedAt,
                    WorkflowRuns::RecordedAt,
                    WorkflowRuns::Seq,
                    WorkflowRuns::Step,
                    WorkflowRuns::TryN,
                    WorkflowRuns::WaitingSince,
                    WorkflowRuns::EndedStep,
                    WorkflowRuns::Plan,
                    WorkflowRuns::Trigger,
                    WorkflowRuns::PullNumber,
                    WorkflowRuns::HeadSha,
                ])
                .values_panic([
                    run.workflow.get().into(),
                    run.repo.to_string().into(),
                    run.due_at.as_millis().into(),
                    cols.box_id.into(),
                    run.state.tag().into(),
                    cols.detail.clone().into(),
                    cols.ended_at.into(),
                    run.recorded_at.as_millis().into(),
                    seq.into(),
                    cols.step.clone().into(),
                    cols.try_n.into(),
                    cols.waiting_since.into(),
                    cols.ended_step.into(),
                    plan.into(),
                    run.cause.trigger().as_str().into(),
                    pull_number.into(),
                    head_sha.into(),
                ])
                .on_conflict(OnConflict::new().do_nothing().to_owned())
                .returning_col(WorkflowRuns::Id),
            |r| r.get::<_, i64>(0),
        )?
        .next()
        .transpose()?;
    let Some(id) = id else {
        return Ok(0);
    };
    let entry = HistoryEntry::moving(RunPhase::Queued, &run.state);
    append_history(tx, id, run.recorded_at, &entry)?;
    Ok(1)
}

/// The run that keeps another of `workflow` from starting: one that is claimed, working or waiting,
/// whenever it was due, else the latest real run when it timed out and its Box is still open.
fn blocking(tx: &dyn Db, workflow: WorkflowId) -> Result<Option<WorkflowRun>, StoreError> {
    let open = tx
        .rows(
            Query::select()
                .columns(super::row::RUN_COLS)
                .from(WorkflowRuns::Table)
                .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                .and_where(Expr::col(WorkflowRuns::Outcome).is_in([STARTING, STARTED, WAITING]))
                .order_by(WorkflowRuns::Id, Order::Desc)
                .limit(1),
            |r| read_run(r, 0),
        )?
        .map(|row| run_of(workflow, &row?))
        .next()
        .transpose()?;
    if open.is_some() {
        return Ok(open);
    }
    let Some(last) = real_run(tx, workflow)? else {
        return Ok(None);
    };
    let RunState::Done(RunEnd::Ended {
        box_id,
        outcome: RunOutcome::TimedOut { .. },
        ..
    }) = last.state
    else {
        return Ok(None);
    };
    Ok(box_open(tx, box_id)?.then_some(last))
}

/// Whether Box `id` exists and is neither closed nor rejected, read in `tx`.
fn box_open(tx: &dyn Db, id: BoxId) -> Result<bool, StoreError> {
    let found: Option<i64> = tx
        .row(
            Query::select()
                .column(BoxState::Box)
                .from(BoxState::Table)
                .and_where(Expr::col(BoxState::Box).eq(id.get()))
                .and_where(Expr::col(BoxState::State).is_not_in([
                    BoxLifecycle::Closed.as_str(),
                    BoxLifecycle::Rejected.as_str(),
                ])),
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}
