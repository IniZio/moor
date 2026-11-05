//! The change stream of a user's workflow runs.

use moor_core::user::UserId;
use moor_core::workflow::{ChangeKind, WorkflowChange, WorkflowId, WorkflowName, WorkflowOrigin};
use moor_schema::tables::{WorkflowRuns, WorkflowSeq, Workflows};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::corrupt;
use super::runs::{run_of, RunRow, RUN_COLS};
use crate::store::db::{self, Db, DbExt as _};
use crate::store::{Store, StoreError};

/// The next change number, taken in `tx`. The counter row stays locked until `tx` ends, so a
/// number committed later is always larger and a poller never steps past an uncommitted one.
fn next_seq(tx: &dyn Db) -> Result<i64, StoreError> {
    tx.run(
        Query::update()
            .table(WorkflowSeq::Table)
            .value(WorkflowSeq::N, Expr::col(WorkflowSeq::N).add(1))
            .and_where(Expr::col(WorkflowSeq::Id).eq(1)),
    )?;
    Ok(tx.row(
        Query::select()
            .column(WorkflowSeq::N)
            .from(WorkflowSeq::Table)
            .and_where(Expr::col(WorkflowSeq::Id).eq(1)),
        |r| r.get(0),
    )?)
}

impl Store {
    /// Runs `write` in a transaction with the next change number; true, and committed, when it
    /// changed one row. Otherwise nothing is kept, the number included.
    pub(super) fn numbered(
        &self,
        write: impl FnOnce(&dyn Db, i64) -> Result<usize, StoreError>,
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
    /// run recorded or moved on, and every workflow the controller disabled.
    pub(crate) fn workflow_changes(
        &self,
        owner: UserId,
        after: i64,
        limit: u64,
    ) -> Result<Vec<WorkflowChange>, StoreError> {
        let mut out = self.run_changes(owner, after, limit)?;
        out.extend(self.disabled_changes(owner, after, limit)?);
        out.sort_by_key(|c| c.seq);
        out.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(out)
    }

    /// The runs of `owner`'s workflows numbered after `after`.
    fn run_changes(
        &self,
        owner: UserId,
        after: i64,
        limit: u64,
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
                .order_by((WorkflowRuns::Table, WorkflowRuns::Seq), Order::Asc)
                .limit(limit),
            |r| {
                let run: RunRow = (
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                );
                Ok::<_, db::Error>((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    run,
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
                what: ChangeKind::Run(run_of(WorkflowId::new(workflow), run)?),
            });
        }
        Ok(out)
    }

    /// The workflows of `owner` the controller disabled, numbered after `after`.
    fn disabled_changes(
        &self,
        owner: UserId,
        after: i64,
        limit: u64,
    ) -> Result<Vec<WorkflowChange>, StoreError> {
        let reader = self.reader()?;
        let disabled = reader.rows(
            Query::select()
                .columns([
                    Workflows::ChangeSeq,
                    Workflows::Name,
                    Workflows::OriginFront,
                    Workflows::OriginKey,
                    Workflows::DisabledReason,
                ])
                .from(Workflows::Table)
                .and_where(Expr::col(Workflows::User).eq(owner.get()))
                .and_where(Expr::col(Workflows::ChangeSeq).gt(after))
                .and_where(Expr::col(Workflows::EnabledAt).is_null())
                .and_where(Expr::col(Workflows::DisabledReason).is_not_null())
                .order_by(Workflows::ChangeSeq, Order::Asc)
                .limit(limit),
            |r| {
                Ok::<_, db::Error>((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in disabled {
            let (seq, name, front, key, reason) = row?;
            out.push(WorkflowChange {
                seq,
                workflow: WorkflowName::parse(&name).map_err(|e| corrupt("name", e))?,
                origin: front.map(|front| WorkflowOrigin { front, key }),
                what: ChangeKind::Disabled { reason },
            });
        }
        Ok(out)
    }
}
