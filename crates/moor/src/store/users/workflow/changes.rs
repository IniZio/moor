//! The change stream of a user's workflow runs.

use moor_core::user::UserId;
use moor_core::workflow::{PullTarget, WorkflowChange, WorkflowId, WorkflowName, WorkflowOrigin};
use moor_schema::tables::{WorkflowRuns, WorkflowSeq, Workflows};
use sea_query::{Condition, Expr, ExprTrait as _, OnConflict, Order, Query};

use super::corrupt;
use super::row::{read_run, run_of, RunRow, RUN_COLS};
use crate::store::db::{self, Db, DbExt as _};
use crate::store::{Store, StoreError};

/// The next change number, taken in `tx`. One upsert creates the counter row or increments it and
/// returns the new number. The row stays locked until `tx` ends, so a number committed later is
/// always larger and a poller never steps past an uncommitted one.
pub(super) fn next_seq(tx: &dyn Db) -> Result<i64, StoreError> {
    Ok(tx.row(
        Query::insert()
            .into_table(WorkflowSeq::Table)
            .columns([WorkflowSeq::Id, WorkflowSeq::N])
            .values_panic([1_i64.into(), 1_i64.into()])
            .on_conflict(
                OnConflict::column(WorkflowSeq::Id)
                    .value(
                        WorkflowSeq::N,
                        Expr::col((WorkflowSeq::Table, WorkflowSeq::N)).add(1),
                    )
                    .to_owned(),
            )
            .returning_col(WorkflowSeq::N),
        |r| r.get(0),
    )?)
}

/// The condition a change stream read with `only` keeps a change by: always, or only the
/// workflow of that name and the runs for that pull request.
fn kept((workflow, pull): (Option<&WorkflowName>, Option<&PullTarget>)) -> Condition {
    let pull_number = pull.and_then(|p| i64::try_from(p.number).ok());
    Condition::all()
        .add_option(workflow.map(|n| Expr::col((Workflows::Table, Workflows::Name)).eq(n.as_str())))
        .add_option(pull.map(|p| {
            Expr::col((WorkflowRuns::Table, WorkflowRuns::Repo))
                .eq(p.repo.to_string())
                .and(Expr::col((WorkflowRuns::Table, WorkflowRuns::PullNumber)).eq(pull_number))
        }))
}

impl Store {
    /// Runs `write` in a transaction with the next change number; true, and committed, when it
    /// changed one row. Otherwise nothing is kept, the number included.
    pub(super) fn numbered(
        &self,
        write: impl FnOnce(&db::Tx<'_>, i64) -> Result<usize, StoreError>,
    ) -> Result<bool, StoreError> {
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let seq = next_seq(&tx)?;
        let changed = write(&tx, seq)?;
        if changed == 1 {
            tx.commit()?;
        } else {
            drop(tx);
        }
        drop(writer);
        Ok(changed == 1)
    }

    /// The changes to `owner`'s workflows after `after`, oldest first, at most `limit`: every
    /// run recorded or moved on. With `only`, just the changes of the workflow of that name and
    /// of the runs for that pull request. The filters are in the query, so `limit` counts only
    /// the changes they keep.
    pub(crate) fn workflow_changes(
        &self,
        owner: UserId,
        after: i64,
        limit: u64,
        only: (Option<&WorkflowName>, Option<&PullTarget>),
    ) -> Result<Vec<WorkflowChange>, StoreError> {
        let reader = self.reader()?;
        let runs = reader.rows(
            Query::select()
                .column((WorkflowRuns::Table, WorkflowRuns::Seq))
                .column((WorkflowRuns::Table, WorkflowRuns::Workflow))
                .column((Workflows::Table, Workflows::Name))
                .column((Workflows::Table, Workflows::OriginFront))
                .column((Workflows::Table, Workflows::OriginKey))
                .columns(RUN_COLS.map(|c| (WorkflowRuns::Table, c)))
                .from(WorkflowRuns::Table)
                .inner_join(
                    Workflows::Table,
                    Expr::col((WorkflowRuns::Table, WorkflowRuns::Workflow))
                        .equals((Workflows::Table, Workflows::Id)),
                )
                .and_where(Expr::col((Workflows::Table, Workflows::User)).eq(owner.get()))
                .and_where(Expr::col((WorkflowRuns::Table, WorkflowRuns::Seq)).gt(after))
                .cond_where(kept(only))
                .order_by((WorkflowRuns::Table, WorkflowRuns::Seq), Order::Asc)
                .limit(limit),
            |r| {
                Ok::<_, db::Error>((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    read_run(r, 5)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in runs {
            let (seq, workflow, name, front, key, run): (
                i64,
                i64,
                String,
                Option<String>,
                Option<String>,
                RunRow,
            ) = row?;
            out.push(WorkflowChange {
                seq,
                workflow: WorkflowName::parse(&name).map_err(|e| corrupt("name", e))?,
                origin: front.map(|front| WorkflowOrigin { front, key }),
                run: Box::new(run_of(WorkflowId::new(workflow), &run)?),
            });
        }
        Ok(out)
    }
}
