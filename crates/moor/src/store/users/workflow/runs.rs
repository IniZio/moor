//! The record of each workflow firing.
//!
//! Every move of a run is one transaction in [`Store::numbered`]: the row changes only if it is
//! still in the state it was read in, and the move's line is appended to the run's history in
//! the same transaction, so a move repeated or raced by another tick changes nothing.

use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    HistoryEntry, NewRun, Plan, RunEnd, RunOutcome, RunPhase, RunState, WorkflowId, WorkflowRun,
};
use moor_core::BoxId;
use moor_schema::tables::WorkflowRuns;
use sea_query::{Expr, ExprTrait as _, Func, OnConflict, Order, Query};

use super::history::{append_history, run_id};
use super::row::{
    read_run, run_of, state_cols, RunRow, REFUSED, RUN_COLS, SKIPPED_OVERLAP, STARTED, STARTING,
    WAITING,
};
use crate::store::db::{self, DbExt as _};
use crate::store::{Store, StoreError};

/// A phase a run can move on from.
#[derive(Debug, Clone, Copy)]
enum Open {
    /// Claimed, its Box not yet recorded.
    Queued,
    /// At a step.
    Running,
}

impl Open {
    /// The `outcome` a run in this phase is stored with.
    const fn outcome(self) -> &'static str {
        match self {
            Self::Queued => STARTING,
            Self::Running => STARTED,
        }
    }

    /// The phase as the history names it.
    const fn phase(self) -> RunPhase {
        match self {
            Self::Queued => RunPhase::Queued,
            Self::Running => RunPhase::Running,
        }
    }
}

impl Store {
    /// Records one firing. False, with nothing changed, when that workflow, repo and due time
    /// already have one, so a firing repeated after a crash records once. The run's history
    /// opens with the line for its state.
    pub(crate) fn record_workflow_run(&self, run: &NewRun) -> Result<bool, StoreError> {
        let cols = state_cols(&run.state);
        let entry = HistoryEntry::moving(RunPhase::Queued, &run.state);
        let plan = run.plan.as_ref().map(Plan::to_json).transpose()?;
        let key = (run.workflow, &run.repo, run.due_at);
        self.numbered(|tx, seq| {
            let inserted = tx.run(
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
                        cols.ended_step.clone().into(),
                        plan.clone().into(),
                    ])
                    .on_conflict(OnConflict::new().do_nothing().to_owned()),
            )?;
            if inserted == 1 {
                append_history(tx, run_id(tx, key)?, run.recorded_at, &entry)?;
            }
            Ok(inserted)
        })
    }

    /// Moves the run of `workflow` and `repo` due at `due_at` from the phase `from` to `to` at
    /// `at`, and appends the line for the move, when the run is still in `from`. False, with
    /// nothing changed, otherwise, so whichever of two moves comes first wins.
    fn move_run(
        &self,
        (workflow, repo, due_at): (WorkflowId, &ForgeRepo, Ts),
        (from, to): (Open, &RunState),
        at: Ts,
    ) -> Result<bool, StoreError> {
        let cols = state_cols(to);
        let entry = HistoryEntry::moving(from.phase(), to);
        self.numbered(|tx, seq| {
            let changed = tx.run(
                Query::update()
                    .table(WorkflowRuns::Table)
                    .value(WorkflowRuns::Outcome, to.tag())
                    .value(WorkflowRuns::Box, cols.box_id)
                    .value(WorkflowRuns::Detail, cols.detail.clone())
                    .value(WorkflowRuns::EndedAt, cols.ended_at)
                    .value(WorkflowRuns::Step, cols.step.clone())
                    .value(WorkflowRuns::TryN, cols.try_n)
                    .value(WorkflowRuns::WaitingSince, cols.waiting_since)
                    .value(WorkflowRuns::EndedStep, cols.ended_step.clone())
                    .value(WorkflowRuns::Seq, seq)
                    .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                    .and_where(Expr::col(WorkflowRuns::Repo).eq(repo.to_string()))
                    .and_where(Expr::col(WorkflowRuns::DueAt).eq(due_at.as_millis()))
                    .and_where(Expr::col(WorkflowRuns::Outcome).eq(from.outcome())),
            )?;
            if changed == 1 {
                let id = run_id(tx, (workflow, repo, due_at))?;
                append_history(tx, id, at, &entry)?;
            }
            Ok(changed)
        })
    }

    /// Ends the running run of `workflow` and `repo` due at `due_at` with `outcome`, at `at`.
    /// False, with nothing changed, when there is no such run or it already ended, so whichever
    /// of a finish and an expiry comes first wins.
    pub(crate) fn end_workflow_run(
        &self,
        key: (WorkflowId, &ForgeRepo, Ts),
        (box_id, outcome): (BoxId, RunOutcome),
        at: Ts,
    ) -> Result<bool, StoreError> {
        let to = RunState::Done(RunEnd::Ended {
            box_id,
            ended_at: at,
            outcome,
        });
        self.move_run(key, (Open::Running, &to), at)
    }

    /// Refuses the run of `workflow` and `repo` due at `due_at` with `detail`, dropping its Box
    /// from the run, when it is still in the state it was seen in: only claimed when
    /// `claimed_only`, else running. False, with nothing changed, otherwise, so a start that
    /// settled in the meantime keeps its outcome.
    pub(crate) fn refuse_run(
        &self,
        key: (WorkflowId, &ForgeRepo, Ts),
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
        self.move_run(key, (from, &to), at)
    }

    /// Settles the claimed run of `workflow` and `repo` due at `due_at` as `state`, which is
    /// Running or a refusal. False, with nothing changed, unless the run is still claimed.
    pub(crate) fn resolve_workflow_start(
        &self,
        key: (WorkflowId, &ForgeRepo, Ts),
        state: &RunState,
        at: Ts,
    ) -> Result<bool, StoreError> {
        self.move_run(key, (Open::Queued, state), at)
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

    /// The latest run of `workflow` for `repo` that was a real attempt, whatever came since: not
    /// skipped and not refused. Its place in the table does not depend on how many skips follow.
    pub(crate) fn latest_real_run(
        &self,
        workflow: WorkflowId,
        repo: &ForgeRepo,
    ) -> Result<Option<WorkflowRun>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .columns(RUN_COLS)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                    .and_where(Expr::col(WorkflowRuns::Repo).eq(repo.to_string()))
                    .and_where(
                        Expr::col(WorkflowRuns::Outcome).is_not_in([SKIPPED_OVERLAP, REFUSED]),
                    )
                    .order_by(WorkflowRuns::DueAt, Order::Desc)
                    .limit(1),
                |r| read_run(r, 0),
            )?
            .map(|row| run_of(workflow, &row?))
            .next()
            .transpose()
    }

    /// The latest `limit` runs of `workflow` for `repo` that were not skipped, newest due time
    /// first, so skips never push a failure out of the window.
    pub(crate) fn recent_unskipped_runs(
        &self,
        workflow: WorkflowId,
        repo: &ForgeRepo,
        limit: u64,
    ) -> Result<Vec<WorkflowRun>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .columns(RUN_COLS)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                    .and_where(Expr::col(WorkflowRuns::Repo).eq(repo.to_string()))
                    .and_where(Expr::col(WorkflowRuns::Outcome).ne(SKIPPED_OVERLAP))
                    .order_by(WorkflowRuns::DueAt, Order::Desc)
                    .limit(limit),
                |r| read_run(r, 0),
            )?
            .map(|row| run_of(workflow, &row?))
            .collect()
    }

    /// The latest `limit` runs of `workflow` for `repo`, newest due time first.
    pub(crate) fn recent_workflow_runs(
        &self,
        workflow: WorkflowId,
        repo: &ForgeRepo,
        limit: u64,
    ) -> Result<Vec<WorkflowRun>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .columns(RUN_COLS)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                    .and_where(Expr::col(WorkflowRuns::Repo).eq(repo.to_string()))
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

    /// The run that is at a step in Box `box_id`, with its workflow; none when the Box has none.
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
                    .and_where(Expr::col(WorkflowRuns::Outcome).eq(STARTED))
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

    /// The latest run of each repo of `workflow` that has one, by due time.
    pub(crate) fn last_workflow_runs(
        &self,
        workflow: WorkflowId,
    ) -> Result<Vec<WorkflowRun>, StoreError> {
        let reader = self.reader()?;
        let latest: Vec<(String, i64)> = reader
            .rows(
                Query::select()
                    .column(WorkflowRuns::Repo)
                    .expr(Func::max(Expr::col(WorkflowRuns::DueAt)))
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                    .group_by_col(WorkflowRuns::Repo)
                    .order_by(WorkflowRuns::Repo, Order::Asc),
                |r| Ok::<_, db::Error>((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )?
            .collect::<Result<_, _>>()?;
        latest
            .into_iter()
            .map(|(repo, due)| {
                let row = reader.row(
                    Query::select()
                        .columns(RUN_COLS)
                        .from(WorkflowRuns::Table)
                        .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                        .and_where(Expr::col(WorkflowRuns::Repo).eq(repo))
                        .and_where(Expr::col(WorkflowRuns::DueAt).eq(due)),
                    |r| read_run(r, 0),
                )?;
                run_of(workflow, &row)
            })
            .collect()
    }
}
