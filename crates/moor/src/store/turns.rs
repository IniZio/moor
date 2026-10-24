//! Turn bookkeeping that must be decided and written in one transaction.

use moor_core::domain::event::{Event, TurnOutcome};
use moor_core::BoxId;
use rusqlite::TransactionBehavior;

use super::apply::insert_event;
use super::{Store, StoreError};

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
        let answered: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM events WHERE box = ?1 AND kind = 'turn_done' \
             AND json_extract(payload, '$.steer') = ?2)",
            (box_id.get(), steer),
            |r| r.get(0),
        )?;
        if answered {
            return Ok(false);
        }
        insert_event(&tx, box_id, &Event::TurnDone { steer, outcome }, now)?;
        tx.commit()?;
        drop(conn);
        Ok(true)
    }
}
