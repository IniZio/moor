//! Recording a firing: a cron one the tick claims, and a manual one a person asks for.
//!
//! Every write is one transaction in [`Store::numbered`], and those are serialized on the change
//! counter, so a read in the transaction cannot be overtaken by another claim.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::workflow::{
    CancelCause, CommitSha, HistoryEntry, NewRun, Plan, RunEnd, RunId, RunOutcome, RunPhase,
    RunState, WorkflowRun,
};
use moor_core::BoxId;
use moor_schema::tables::{BoxState, WorkflowRuns};
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use super::changes::next_seq;
use super::history::append_history;
use super::row::{state_cols, subject_cols};
use super::runs::real_run;
use super::subject::{cancel_in, open_run, subject_of};
use crate::store::db::{Db, DbExt as _, OptionalExtension as _};
use crate::store::{Store, StoreError};

/// What asking for a manual run came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualClaim {
    /// The run is recorded, with this id. `cancelled` is the open run of an older head of the
    /// same pull request that the claim ended `Cancelled`, as it was before, whose agent is
    /// still to stop.
    Recorded {
        /// The new run.
        run: RunId,
        /// The run it cancelled.
        cancelled: Option<Box<WorkflowRun>>,
    },
    /// A run about the same subject is open, so nothing was recorded. It is that run.
    Open(Box<WorkflowRun>),
}

impl Store {
    /// Records one firing. False, with nothing changed, when a unique key already holds it: a cron
    /// firing on its workflow, repo and due time, a pull request run on its pull request and head.
    /// A firing repeated after a crash then records once. The run's history opens with the line
    /// for its state.
    #[cfg(test)]
    pub(crate) fn record_workflow_run(&self, run: &NewRun) -> Result<bool, StoreError> {
        Ok(self.claim_workflow_run(run)?.is_some())
    }

    /// Records one firing under the same unique keys and gives the id of the run it recorded, or
    /// none when a key already holds it.
    pub(crate) fn claim_workflow_run(&self, run: &NewRun) -> Result<Option<RunId>, StoreError> {
        let mut id = None;
        self.numbered(|tx, seq| {
            id = insert(tx, seq, run)?;
            Ok(usize::from(id.is_some()))
        })?;
        Ok(id)
    }

    /// Records a run its owner asked for, unless a run of the workflow about the same subject is
    /// open, which is looked for in the transaction that inserts the row. A read before the write
    /// would race the tick, which could claim a cron run with another due time in between. No
    /// unique key covers a manual run, so the open run is the only thing that stops it.
    ///
    /// A run for a pull request whose open run is at an older head cancels that run with
    /// [`CancelCause::NewerHead`] in the same transaction, then records. An older run still
    /// being started has no Box to stop yet, so it blocks as any open run does.
    pub(crate) fn claim_manual_run(&self, run: &NewRun) -> Result<ManualClaim, StoreError> {
        let mut open = None;
        let mut cancelled = None;
        let mut id = None;
        self.numbered(|tx, seq| {
            let Some(old) = blocking(tx, run)? else {
                id = insert(tx, seq, run)?;
                return Ok(usize::from(id.is_some()));
            };
            let superseded = match newer_head(&old, run) {
                Some(sha) => {
                    let cause = CancelCause::NewerHead { sha };
                    cancel_in(tx, seq, &old, cause, run.recorded_at)? == 1
                }
                None => false,
            };
            if !superseded {
                open = Some(old);
                return Ok(0);
            }
            cancelled = Some(old);
            // The cancel holds `seq`; a number of its own keeps a paged change stream from
            // splitting the two.
            id = insert(tx, next_seq(tx)?, run)?;
            Ok(usize::from(id.is_some()))
        })?;
        match (open, id) {
            (Some(run), _) => Ok(ManualClaim::Open(Box::new(run))),
            (None, Some(run)) => Ok(ManualClaim::Recorded {
                run,
                cancelled: cancelled.map(Box::new),
            }),
            (None, None) => Err(StoreError::Invalid(
                "a manual run met a unique key it has no part in".to_owned(),
            )),
        }
    }
}

/// The head `run` moves its pull request to, when `open` is that pull request's run at an
/// older head; none otherwise.
fn newer_head(open: &WorkflowRun, run: &NewRun) -> Option<CommitSha> {
    let (old, new) = (open.cause.subject()?, run.cause.subject()?);
    (old.head_sha != new.head_sha).then(|| new.head_sha.clone())
}

/// Inserts `run` and the first line of its history; its id, none when a unique key already
/// holds it.
fn insert(tx: &dyn Db, seq: i64, run: &NewRun) -> Result<Option<RunId>, StoreError> {
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
        return Ok(None);
    };
    let entry = HistoryEntry::moving(RunPhase::Queued, &run.state);
    append_history(tx, id, run.recorded_at, &entry)?;
    Ok(Some(RunId::new(id)))
}

/// The run that keeps `run` from starting: the run of its workflow about the same subject that
/// is claimed, working or waiting, whenever it was due, else the latest real one when it timed
/// out and its Box is still open. A timed-out run of an older head of a pull request is over and
/// blocks nothing.
fn blocking(tx: &dyn Db, run: &NewRun) -> Result<Option<WorkflowRun>, StoreError> {
    let subject = subject_of(&run.cause);
    if let Some(open) = open_run(tx, run.workflow, subject)? {
        return Ok(Some(open));
    }
    let Some(last) = real_run(tx, run.workflow, subject)? else {
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
    if newer_head(&last, run).is_some() {
        return Ok(None);
    }
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
