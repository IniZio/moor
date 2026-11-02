//! Reads of one kind of a Box's events.

use moor_core::BoxId;

use super::{event_from_row, EventRow, Store, StoreError};

impl Store {
    /// The newest `n` events of `kind` for a box, ascending by id; empty when it has had none.
    pub(crate) fn newest_events_of(
        &self,
        id: BoxId,
        kind: &str,
        n: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        let limit = i64::try_from(n).unwrap_or(i64::MAX);
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, box, kind, payload, ts FROM \
             (SELECT id, box, kind, payload, ts FROM events \
              WHERE box = ?1 AND kind = ?2 ORDER BY id DESC LIMIT ?3) ORDER BY id",
        )?;
        let rows = stmt.query_and_then((id.get(), kind, limit), event_from_row)?;
        rows.collect()
    }
}
