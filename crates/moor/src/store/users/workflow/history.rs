//! The history of a run: lines only appended, each in the transaction of the move it records.

use moor_core::seam::clock::Ts;
use moor_core::workflow::{HistoryEntry, RunId};
use moor_core::BoxId;
use moor_schema::tables::{WorkflowRunHistory, WorkflowRuns};
use sea_query::{Expr, ExprTrait as _, Func, Order, Query};

use super::corrupt;
use super::row::{STARTED, WAITING};
use crate::store::db::{self, Db, DbExt as _, OptionalExtension as _, Tx};
use crate::store::{Store, StoreError};

/// Appends `entry` at `at` to the history of run `run`, as the line after its last.
pub(super) fn append_history(
    tx: &dyn Db,
    run: i64,
    at: Ts,
    entry: &HistoryEntry,
) -> Result<(), StoreError> {
    let n: i64 = tx.row(
        Query::select()
            .expr(
                Func::coalesce([
                    Func::max(Expr::col(WorkflowRunHistory::N)).into(),
                    Expr::value(0_i64),
                ])
                .add(1_i64),
            )
            .from(WorkflowRunHistory::Table)
            .and_where(Expr::col(WorkflowRunHistory::Run).eq(run)),
        |r| r.get(0),
    )?;
    tx.run(
        Query::insert()
            .into_table(WorkflowRunHistory::Table)
            .columns([
                WorkflowRunHistory::Run,
                WorkflowRunHistory::N,
                WorkflowRunHistory::At,
                WorkflowRunHistory::Entry,
            ])
            .values_panic([
                run.into(),
                n.into(),
                at.as_millis().into(),
                serde_json::to_string(entry)?.into(),
            ]),
    )?;
    Ok(())
}

/// Appends the lines `lines` makes to the history of the run at a step in Box `box_id`, running
/// or waiting, in `tx`; it is given whether the run waits. Nothing is written for a Box with no
/// such run.
pub(in crate::store) fn append_to_open_run(
    tx: &Tx<'_>,
    box_id: BoxId,
    at: Ts,
    lines: impl FnOnce(bool) -> Vec<HistoryEntry>,
) -> Result<(), StoreError> {
    let open: Option<(i64, String)> = tx
        .row(
            Query::select()
                .columns([WorkflowRuns::Id, WorkflowRuns::Outcome])
                .from(WorkflowRuns::Table)
                .and_where(Expr::col(WorkflowRuns::Box).eq(box_id.get()))
                .and_where(Expr::col(WorkflowRuns::Outcome).is_in([STARTED, WAITING]))
                .order_by(WorkflowRuns::Id, Order::Desc)
                .limit(1),
            |r| Ok::<_, db::Error>((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((run, outcome)) = open else {
        return Ok(());
    };
    for line in lines(outcome == WAITING) {
        append_history(tx, run, at, &line)?;
    }
    Ok(())
}

/// One line of a run's history as read: its number, when it was written, and its JSON. A kind
/// of line this build does not know stays JSON, so it still shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryLine {
    /// The line's place in the history, from 1.
    pub(crate) n: i64,
    /// When it was written.
    pub(crate) at: Ts,
    /// The line, tagged by its `kind`.
    pub(crate) entry: serde_json::Value,
}

impl Store {
    /// The history of run `run`, oldest line first.
    pub(crate) fn run_history(&self, run: RunId) -> Result<Vec<HistoryLine>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .columns([
                        WorkflowRunHistory::N,
                        WorkflowRunHistory::At,
                        WorkflowRunHistory::Entry,
                    ])
                    .from(WorkflowRunHistory::Table)
                    .and_where(Expr::col(WorkflowRunHistory::Run).eq(run.get()))
                    .order_by(WorkflowRunHistory::N, Order::Asc),
                |r| {
                    Ok::<_, db::Error>((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?
            .map(|row| {
                let (n, at, entry) = row?;
                Ok(HistoryLine {
                    n,
                    at: Ts::from_millis(at),
                    entry: serde_json::from_str(&entry).map_err(|e| corrupt("run history", e))?,
                })
            })
            .collect()
    }
}
