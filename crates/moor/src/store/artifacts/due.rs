//! Durable "collection is due" tracking: a Box is due when a turn ended after its last collection.

use moor_core::BoxId;
use rusqlite::{params, OptionalExtension};

use crate::store::{Store, StoreError};

/// Newest `idled` or `turn_done` event id of the Box bound as `?1`.
const NEWEST_TURN_EVENT: &str =
    "SELECT MAX(id) FROM events WHERE box = ?1 AND kind IN ('idled', 'turn_done')";

impl Store {
    /// Ready, handled, intent-free open Boxes whose newest turn-end event is past their
    /// collection cursor, as (Box, that event id), ordered by Box id.
    pub(crate) fn artifacts_due(&self) -> Result<Vec<(BoxId, i64)>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT b.id, e.newest FROM boxes b \
             JOIN (SELECT box, MAX(id) AS newest FROM events \
                   WHERE kind IN ('idled', 'turn_done') GROUP BY box) e ON e.box = b.id \
             LEFT JOIN artifact_cursors c ON c.box = b.id \
             WHERE b.state = 'ready' AND b.handle IS NOT NULL AND b.intent_kind IS NULL \
               AND e.newest > COALESCE(c.through, 0) \
             ORDER BY b.id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        let mut due = Vec::new();
        for row in rows {
            let (id, newest) = row?;
            due.push((BoxId::new(id), newest));
        }
        Ok(due)
    }

    /// Ready, handled, intent-free open Boxes, by Box id; a Box mid-turn is among them.
    pub(crate) fn artifacts_ready(&self) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id FROM boxes WHERE state = 'ready' AND handle IS NOT NULL \
             AND intent_kind IS NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(BoxId::new(row?));
        }
        Ok(ids)
    }

    /// Newest `idled` or `turn_done` event id of the Box, none when it has had neither.
    pub(crate) fn newest_turn_event(&self, id: BoxId) -> Result<Option<i64>, StoreError> {
        let conn = self.reader()?;
        let newest: Option<i64> = conn
            .query_row(NEWEST_TURN_EVENT, params![id.get()], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(newest)
    }

    /// Records that artifacts were collected through event `through`; never moves backwards.
    pub(crate) fn set_artifacts_through(&self, id: BoxId, through: i64) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.execute(
            "INSERT INTO artifact_cursors (box, through) VALUES (?1, ?2) \
             ON CONFLICT(box) DO UPDATE SET through = MAX(through, excluded.through)",
            params![id.get(), through],
        )?;
        drop(conn);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
