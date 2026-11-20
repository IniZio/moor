//! The history of a run: lines only appended, each in the transaction of the move it records.

use moor_core::seam::clock::Ts;
use moor_core::workflow::{HistoryEntry, RunId};
use moor_schema::tables::WorkflowRunHistory;
use sea_query::{Expr, ExprTrait as _, Func, Order, Query};

use super::corrupt;
use crate::store::db::{self, Db, DbExt as _};
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
