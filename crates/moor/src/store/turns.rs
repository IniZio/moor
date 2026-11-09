//! Turn and link bookkeeping that must be decided and written in one transaction.

use moor_core::domain::event::{Event, TurnOutcome};
use moor_core::BoxId;
use moor_schema::tables::BoxState;
use sea_query::{Expr, ExprTrait as _, Query};

use super::apply::insert_event;
use super::reads::steer_answered;
use super::{Store, StoreError};
use crate::store::db::DbExt as _;

impl Store {
    /// Write a `turn_done` for `steer` unless the Box already has one for it, and make the Box's
    /// archive due.
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
        self.check_write(box_id);
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        if steer_answered(&tx, box_id, steer)? {
            return Ok(false);
        }
        insert_event(&tx, box_id, &Event::TurnDone { steer, outcome }, now)?;
        tx.run(
            Query::update()
                .table(BoxState::Table)
                .value(BoxState::ArchiveDue, now.as_millis())
                .and_where(Expr::col(BoxState::Box).eq(box_id.get())),
        )?;
        tx.commit()?;
        drop(conn);
        Ok(true)
    }
}
