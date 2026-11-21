//! The record of each workflow firing.
//!
//! Every move of a run is one transaction in [`Store::numbered`]: the row changes only if it is
//! still in the state it was read in, and the move's line is appended to the run's history in
//! the same transaction, so a move repeated or raced by another tick changes nothing.

use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    HistoryEntry, RunEnd, RunId, RunOutcome, RunPhase, RunState, RunTrigger, StepId, WorkflowId,
    WorkflowRun,
};
use moor_core::BoxId;
use moor_schema::tables::WorkflowRuns;
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::history::append_history;
use super::notice;
use super::row::{
    read_run, run_of, state_cols, RunRow, FINISHED, REFUSED, RUN_COLS, SKIPPED_FORK,
    SKIPPED_OVERLAP, STARTED, STARTING, WAITING,
};
use super::subject::{about, Subject};
use crate::store::db::{self, Db, DbExt as _, Tx};
use crate::store::{Store, StoreError};

/// A phase a run can move on from.
#[derive(Debug, Clone, Copy)]
pub(super) enum Open {
    /// Claimed, its Box not yet recorded.
    Queued,
    /// At a step.
    Running,
    /// At an approve step, waiting for the owner.
    Waiting,
}

impl Open {
    /// The `outcome` a run in this phase is stored with.
    const fn outcome(self) -> &'static str {
        match self {
            Self::Queued => STARTING,
            Self::Running => STARTED,
            Self::Waiting => WAITING,
        }
    }

    /// The phase as the history names it.
    const fn phase(self) -> RunPhase {
        match self {
            Self::Queued => RunPhase::Queued,
            Self::Running => RunPhase::Running,
            Self::Waiting => RunPhase::Waiting,
        }
    }
}

/// How a Box relates to the workflow runs that used it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunOfBox {
    /// No run used it: a one-off Box.
    NoRun,
    /// Its latest run is being started, works or waits for an answer.
    Open,
    /// Its latest run ended at this time.
    Ended(Ts),
}

impl Store {
    /// How Box `box_id` relates to its latest workflow run.
    pub(crate) fn run_of_box(&self, box_id: BoxId) -> Result<RunOfBox, StoreError> {
        Ok(match self.latest_run_of_box(box_id)? {
            None => RunOfBox::NoRun,
            Some((outcome, _)) if [STARTING, STARTED, WAITING].contains(&outcome.as_str()) => {
                RunOfBox::Open
            }
            Some((_, Some(ended))) => RunOfBox::Ended(Ts::from_millis(ended)),
            Some((_, None)) => RunOfBox::Open,
        })
    }

    /// How the latest workflow run of Box `box_id` ended short of delivering, as its stored
    /// outcome such as `cancelled` or `timed_out`; none while it is open, once it delivered, or
    /// for a Box no run used.
    pub(crate) fn run_end_of_box(&self, box_id: BoxId) -> Result<Option<String>, StoreError> {
        let not_ended = [STARTING, STARTED, WAITING, FINISHED];
        Ok(match self.latest_run_of_box(box_id)? {
            Some((outcome, Some(_))) if !not_ended.contains(&outcome.as_str()) => Some(outcome),
            _ => None,
        })
    }

    /// The outcome and end time of the latest workflow run of Box `box_id`.
    fn latest_run_of_box(
        &self,
        box_id: BoxId,
    ) -> Result<Option<(String, Option<i64>)>, StoreError> {
        let latest = self
            .reader()?
            .rows(
                Query::select()
                    .columns([WorkflowRuns::Outcome, WorkflowRuns::EndedAt])
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Box).eq(box_id.get()))
                    .order_by(WorkflowRuns::Id, Order::Desc)
                    .limit(1),
                |r| {
                    let outcome: String = r.get(0)?;
                    let ended: Option<i64> = r.get(1)?;
                    Ok::<_, db::Error>((outcome, ended))
                },
            )?
            .next()
            .transpose()?;
        Ok(latest)
    }

    /// Moves run `run` from the phase `from` to `to` at `at`, and appends the line for the
    /// move, when the run is still in `from`. False, with
    /// nothing changed, otherwise, so whichever of two moves comes first wins.
    ///
    /// `guard` also requires a running or waiting run to be at exactly that step and try, and
    /// `lines` are appended before the move's own line, in the same transaction. A move that
    /// enters a step passes the `Entered` line in `lines` and sets `own` false, so the line is
    /// not written twice.
    pub(super) fn move_run(
        &self,
        run: RunId,
        (from, to): (Open, &RunState),
        (guard, lines, at, own): (Option<(&StepId, u8)>, &[HistoryEntry], Ts, bool),
    ) -> Result<bool, StoreError> {
        self.numbered(|tx, seq| move_in(tx, seq, run, (from, to), (guard, lines, at, own)))
    }

    /// Ends run `run` with `outcome`, at `at`, appending
    /// `lines` first, when it is running at exactly step `on` and try `on.1`. False, with nothing
    /// changed, when there is no such run, it ended, or it moved to another step or try, so
    /// whichever of a finish, a report and an expiry comes first wins.
    pub(crate) fn end_workflow_run(
        &self,
        key: RunId,
        on: (&StepId, u8),
        (box_id, outcome): (BoxId, RunOutcome),
        (lines, at): (&[HistoryEntry], Ts),
    ) -> Result<bool, StoreError> {
        let to = RunState::Done(RunEnd::Ended {
            box_id,
            ended_at: at,
            outcome,
        });
        self.move_run(key, (Open::Running, &to), (Some(on), lines, at, true))
    }

    /// Refuses run `run` with `detail`, dropping its Box
    /// from the run, when it is still in the state it was seen in: only claimed when
    /// `claimed_only`, else running. False, with nothing changed, otherwise, so a start that
    /// settled in the meantime keeps its outcome.
    pub(crate) fn refuse_run(
        &self,
        key: RunId,
        (detail, claimed_only): (&str, bool),
        at: Ts,
    ) -> Result<bool, StoreError> {
        let from = if claimed_only {
            Open::Queued
        } else {
            Open::Running
        };
        let to = RunState::Done(RunEnd::Refused {
            detail: detail.to_owned(),
        });
        self.move_run(key, (from, &to), (None, &[], at, true))
    }

    /// Settles the claimed run `run` as `state`, which is
    /// Running or a refusal. False, with nothing changed, unless the run is still claimed.
    pub(crate) fn resolve_workflow_start(
        &self,
        key: RunId,
        state: &RunState,
        at: Ts,
    ) -> Result<bool, StoreError> {
        self.move_run(key, (Open::Queued, state), (None, &[], at, true))
    }

    /// The runs of `workflow` that are being started, work, or wait: those that have not ended.
    pub(crate) fn open_workflow_runs(
        &self,
        workflow: WorkflowId,
    ) -> Result<Vec<WorkflowRun>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .columns(RUN_COLS)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                    .and_where(Expr::col(WorkflowRuns::Outcome).is_in([STARTING, STARTED, WAITING]))
                    .order_by(WorkflowRuns::DueAt, Order::Asc),
                |r| read_run(r, 0),
            )?
            .map(|row| run_of(workflow, &row?))
            .collect()
    }

    /// The latest run of `workflow` for no pull request that was a real attempt, whatever came
    /// since: not skipped and not refused. A pull request's runs never hold back the schedule. Its place in the table does not depend on how many skips follow.
    /// Latest is the last recorded, not the latest due: a run fired by hand is due when it is
    /// claimed, so a cron run for an earlier due time can be recorded after it.
    pub(crate) fn latest_real_run(
        &self,
        workflow: WorkflowId,
    ) -> Result<Option<WorkflowRun>, StoreError> {
        real_run(&self.reader()?, workflow, None)
    }

    /// The latest cron run of `workflow` by due time, whatever came of it. A run
    /// fired by hand is left out, so it never hides a due time the schedule missed.
    pub(crate) fn latest_cron_run(
        &self,
        workflow: WorkflowId,
    ) -> Result<Option<WorkflowRun>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .columns(RUN_COLS)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                    .and_where(Expr::col(WorkflowRuns::Trigger).eq(RunTrigger::Cron.as_str()))
                    .order_by(WorkflowRuns::DueAt, Order::Desc)
                    .limit(1),
                |r| read_run(r, 0),
            )?
            .map(|row| run_of(workflow, &row?))
            .next()
            .transpose()
    }

    /// The latest `limit` runs of `workflow`, newest due time first.
    pub(crate) fn recent_workflow_runs(
        &self,
        workflow: WorkflowId,
        limit: u64,
    ) -> Result<Vec<WorkflowRun>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .columns(RUN_COLS)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                    .order_by(WorkflowRuns::DueAt, Order::Desc)
                    .limit(limit),
                |r| read_run(r, 0),
            )?
            .map(|row| run_of(workflow, &row?))
            .collect()
    }

    /// Every claimed run whose Box was not yet recorded, of any workflow.
    pub(crate) fn starting_workflow_runs(&self) -> Result<Vec<WorkflowRun>, StoreError> {
        let reader = self.reader()?;
        let rows: Vec<(i64, RunRow)> = reader
            .rows(
                Query::select()
                    .column(WorkflowRuns::Workflow)
                    .columns(RUN_COLS)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Outcome).eq(STARTING))
                    .order_by(WorkflowRuns::Id, Order::Asc),
                |r| Ok::<_, db::Error>((r.get(0)?, read_run(r, 1)?)),
            )?
            .collect::<Result<_, _>>()?;
        rows.into_iter()
            .map(|(workflow, row)| run_of(WorkflowId::new(workflow), &row))
            .collect()
    }

    /// The run that is at a step in Box `box_id`, running or waiting, with its workflow; none when
    /// the Box has none.
    pub(crate) fn running_run_of_box(
        &self,
        box_id: BoxId,
    ) -> Result<Option<(WorkflowId, WorkflowRun)>, StoreError> {
        let row: Option<(i64, RunRow)> = self
            .reader()?
            .rows(
                Query::select()
                    .column(WorkflowRuns::Workflow)
                    .columns(RUN_COLS)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Box).eq(box_id.get()))
                    .and_where(Expr::col(WorkflowRuns::Outcome).is_in([STARTED, WAITING]))
                    .order_by(WorkflowRuns::Id, Order::Desc)
                    .limit(1),
                |r| Ok::<_, db::Error>((r.get(0)?, read_run(r, 1)?)),
            )?
            .next()
            .transpose()?;
        row.map(|(workflow, row)| {
            let id = WorkflowId::new(workflow);
            run_of(id, &row).map(|run| (id, run))
        })
        .transpose()
    }

    /// The latest run of `workflow` by due time, when it has one.
    pub(crate) fn last_workflow_run(
        &self,
        workflow: WorkflowId,
    ) -> Result<Option<WorkflowRun>, StoreError> {
        Ok(self.recent_workflow_runs(workflow, 1)?.pop())
    }
}

/// [`Store::move_run`] in `tx`, numbered `seq`; the rows it changed.
pub(super) fn move_in(
    tx: &Tx<'_>,
    seq: i64,
    run: RunId,
    (from, to): (Open, &RunState),
    (guard, lines, at, own): (Option<(&StepId, u8)>, &[HistoryEntry], Ts, bool),
) -> Result<usize, StoreError> {
    let cols = state_cols(to);
    let entry = HistoryEntry::moving(from.phase(), to);
    let Some(held) = notice::held(tx, run)? else {
        return Ok(0);
    };
    let mut update = Query::update();
    update
        .table(WorkflowRuns::Table)
        .value(WorkflowRuns::Outcome, to.tag())
        .value(WorkflowRuns::Box, cols.box_id)
        .value(WorkflowRuns::Detail, cols.detail)
        .value(WorkflowRuns::EndedAt, cols.ended_at)
        .value(WorkflowRuns::Step, cols.step)
        .value(WorkflowRuns::TryN, cols.try_n)
        .value(WorkflowRuns::WaitingSince, cols.waiting_since)
        .value(WorkflowRuns::EndedStep, cols.ended_step)
        .value(WorkflowRuns::Seq, seq)
        .and_where(Expr::col(WorkflowRuns::Id).eq(run.get()))
        .and_where(Expr::col(WorkflowRuns::Outcome).eq(from.outcome()));
    if let Some((step, try_n)) = guard {
        update
            .and_where(Expr::col(WorkflowRuns::Step).eq(step.as_str()))
            .and_where(Expr::col(WorkflowRuns::TryN).eq(i64::from(try_n)));
    }
    let changed = tx.run(&mut update)?;
    if changed == 1 {
        for line in lines {
            append_history(tx, run.get(), at, line)?;
        }
        if own {
            append_history(tx, run.get(), at, &entry)?;
        }
        notice::record(tx, run, &held, to, at)?;
    }
    Ok(changed)
}

/// The latest run of `workflow` about `subject` that was a real attempt, read through `db`,
/// which may be a transaction.
pub(super) fn real_run(
    db: &dyn Db,
    workflow: WorkflowId,
    subject: Subject<'_>,
) -> Result<Option<WorkflowRun>, StoreError> {
    db.rows(
        Query::select()
            .columns(RUN_COLS)
            .from(WorkflowRuns::Table)
            .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
            .and_where(Expr::col(WorkflowRuns::Outcome).is_not_in([
                SKIPPED_OVERLAP,
                SKIPPED_FORK,
                REFUSED,
            ]))
            .cond_where(about(subject))
            .order_by(WorkflowRuns::Id, Order::Desc)
            .limit(1),
        |r| read_run(r, 0),
    )?
    .map(|row| run_of(workflow, &row?))
    .next()
    .transpose()
}
