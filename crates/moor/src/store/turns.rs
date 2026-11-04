//! Turn and link bookkeeping that must be decided and written in one transaction.

use moor_core::domain::event::{Event, TurnOutcome};
use moor_core::BoxId;
use moor_schema::tables::Launches;
use sea_query::{Expr, ExprTrait as _, Query};

use super::apply::insert_event;
use super::reads::steer_answered;
use super::{Store, StoreError};
use crate::store::db::{DbExt as _, TransactionBehavior};

impl Store {
    /// Write a `turn_done` for `steer` unless the Box already has one for it.
    ///
    /// The check and the insert share one immediate transaction, so concurrent callers
    /// cannot both write. Returns whether this call wrote the event.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    pub(crate) fn record_turn_done(
        &self,
        box_id: BoxId,
        steer: i64,
        outcome: TurnOutcome,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if steer_answered(&tx, box_id, steer)? {
            return Ok(false);
        }
        insert_event(&tx, box_id, &Event::TurnDone { steer, outcome }, now)?;
        tx.commit()?;
        drop(conn);
        Ok(true)
    }

    /// Moves the live launch `launch` of `box_id` to `pane` after its old pane ended with
    /// `exit` while the agent kept running, and records the lost and restored link.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    pub(crate) fn reattach_pane(
        &self,
        box_id: BoxId,
        launch: i64,
        exit: Option<i32>,
        pane: &str,
    ) -> Result<(), StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<String> = tx.row(
            Query::select()
                .column(Launches::Pane)
                .from(Launches::Table)
                .and_where(Expr::col(Launches::Id).eq(launch)),
            |r| r.get(0),
        )?;
        tx.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::Pane, pane)
                .and_where(Expr::col(Launches::Id).eq(launch)),
        )?;
        insert_event(&tx, box_id, &Event::AgentLinkLost { pane: old, exit }, now)?;
        let restored = Event::AgentLinkRestored {
            pane: pane.to_owned(),
        };
        insert_event(&tx, box_id, &restored, now)?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }
}
