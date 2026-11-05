//! The record of each workflow firing.

use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{RunEnd, WorkflowId, WorkflowOutcome, WorkflowRun};
use moor_core::BoxId;
use moor_schema::tables::WorkflowRuns;
use sea_query::{Expr, ExprTrait as _, Func, OnConflict, Order, Query};

use super::corrupt;
use crate::store::db::{self, DbExt as _};
use crate::store::{Store, StoreError};

/// The `outcome` of a run that started a Box.
const STARTED: &str = "started";
/// The `outcome` of a run whose Box finished in time.
const FINISHED: &str = "finished";
/// The `outcome` of a run expired at its timeout.
const TIMED_OUT: &str = "timed_out";
/// The `outcome` of a run skipped for an open previous Box.
const SKIPPED_OVERLAP: &str = "skipped_overlap";
/// The `outcome` of a run whose start was refused.
const REFUSED: &str = "refused";

/// A `workflow_runs` row as read: repo, due time, box, outcome, detail, end, recorded time.
type RunRow = (
    String,
    i64,
    Option<i64>,
    String,
    Option<String>,
    Option<i64>,
    i64,
);

/// The columns of a `workflow_runs` row, in the order [`read_run`] reads them.
const RUN_COLS: [WorkflowRuns; 7] = [
    WorkflowRuns::Repo,
    WorkflowRuns::DueAt,
    WorkflowRuns::Box,
    WorkflowRuns::Outcome,
    WorkflowRuns::Detail,
    WorkflowRuns::EndedAt,
    WorkflowRuns::RecordedAt,
];

/// Reads one `workflow_runs` row.
fn read_run(r: &db::Row) -> Result<RunRow, db::Error> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
    ))
}

/// The run a stored row holds; an outcome whose box, detail or end do not fit it is corrupt.
fn run_of(workflow: WorkflowId, row: RunRow) -> Result<WorkflowRun, StoreError> {
    let (repo, due_at, box_id, outcome, detail, ended_at, recorded_at) = row;
    let box_id = box_id.map(BoxId::new);
    let ended_at = ended_at.map(Ts::from_millis);
    let outcome = match (outcome.as_str(), box_id, detail, ended_at) {
        (STARTED, Some(box_id), None, None) => WorkflowOutcome::Started { box_id },
        (FINISHED, Some(box_id), None, Some(ended_at)) => {
            WorkflowOutcome::Finished { box_id, ended_at }
        }
        (TIMED_OUT, Some(box_id), None, Some(ended_at)) => {
            WorkflowOutcome::TimedOut { box_id, ended_at }
        }
        (SKIPPED_OVERLAP, None, None, None) => WorkflowOutcome::SkippedOverlap,
        (REFUSED, None, Some(detail), None) => WorkflowOutcome::Refused { detail },
        (other, ..) => return Err(corrupt("run outcome", other)),
    };
    Ok(WorkflowRun {
        workflow,
        repo: ForgeRepo::parse(&repo).map_err(|e| corrupt("run repo", e))?,
        due_at: Ts::from_millis(due_at),
        outcome,
        recorded_at: Ts::from_millis(recorded_at),
    })
}

/// The stored form of an outcome: its name, box, detail and end.
const fn outcome_cols(
    o: &WorkflowOutcome,
) -> (&'static str, Option<i64>, Option<&str>, Option<i64>) {
    match o {
        WorkflowOutcome::Started { box_id } => (STARTED, Some(box_id.get()), None, None),
        WorkflowOutcome::Finished { box_id, ended_at } => (
            FINISHED,
            Some(box_id.get()),
            None,
            Some(ended_at.as_millis()),
        ),
        WorkflowOutcome::TimedOut { box_id, ended_at } => (
            TIMED_OUT,
            Some(box_id.get()),
            None,
            Some(ended_at.as_millis()),
        ),
        WorkflowOutcome::SkippedOverlap => (SKIPPED_OVERLAP, None, None, None),
        WorkflowOutcome::Refused { detail } => (REFUSED, None, Some(detail.as_str()), None),
    }
}

impl Store {
    /// Records one firing. False, with nothing changed, when that workflow, repo and due time
    /// already have one, so a firing repeated after a crash records once.
    pub(crate) fn record_workflow_run(&self, run: &WorkflowRun) -> Result<bool, StoreError> {
        let (outcome, box_id, detail, ended_at) = outcome_cols(&run.outcome);
        let added = self.writer().run(
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
                ])
                .values_panic([
                    run.workflow.get().into(),
                    run.repo.to_string().into(),
                    run.due_at.as_millis().into(),
                    box_id.into(),
                    outcome.into(),
                    detail.into(),
                    ended_at.into(),
                    run.recorded_at.as_millis().into(),
                ])
                .on_conflict(OnConflict::new().do_nothing().to_owned()),
        )?;
        Ok(added == 1)
    }

    /// Ends the started run of `workflow` and `repo` due at `due_at`, at `at`. False, with
    /// nothing changed, when there is no such run or it already ended, so whichever of a finish
    /// and an expiry comes first wins.
    pub(crate) fn end_workflow_run(
        &self,
        (workflow, repo, due_at): (WorkflowId, &ForgeRepo, Ts),
        end: RunEnd,
        at: Ts,
    ) -> Result<bool, StoreError> {
        let outcome = match end {
            RunEnd::Finished => FINISHED,
            RunEnd::TimedOut => TIMED_OUT,
        };
        let changed = self.writer().run(
            Query::update()
                .table(WorkflowRuns::Table)
                .value(WorkflowRuns::Outcome, outcome)
                .value(WorkflowRuns::EndedAt, at.as_millis())
                .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                .and_where(Expr::col(WorkflowRuns::Repo).eq(repo.to_string()))
                .and_where(Expr::col(WorkflowRuns::DueAt).eq(due_at.as_millis()))
                .and_where(Expr::col(WorkflowRuns::Outcome).eq(STARTED)),
        )?;
        Ok(changed == 1)
    }

    /// The runs of `workflow` that started a Box and have not ended.
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
                    .and_where(Expr::col(WorkflowRuns::Outcome).eq(STARTED))
                    .order_by(WorkflowRuns::DueAt, Order::Asc),
                read_run,
            )?
            .map(|row| run_of(workflow, row?))
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
                read_run,
            )?
            .map(|row| run_of(workflow, row?))
            .collect()
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
                    read_run,
                )?;
                run_of(workflow, row)
            })
            .collect()
    }
}
