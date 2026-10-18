//! Sessions of a Box beyond the primary one inserted with it.

use moor_core::domain::session_state::SessionState;
use moor_core::BoxId;
use rusqlite::{OptionalExtension, TransactionBehavior};

use super::{parsed_opt, Store, StoreError};

/// One Session of a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoxSession {
    /// Row id of the Session.
    pub(crate) id: i64,
    /// Position among the Box's Sessions, starting at 1.
    pub(crate) ordinal: i64,
    /// Where the Session is in its life.
    pub(crate) state: SessionState,
}

impl Store {
    /// Open another Session on Box `id`, numbered after its last one, in the Starting state.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] when the Box does not exist, or [`StoreError`] when the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn open_session(&self, id: BoxId) -> Result<BoxSession, StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists = tx
            .query_row("SELECT 1 FROM boxes WHERE id = ?1", [id.get()], |_| Ok(()))
            .optional()?;
        if exists.is_none() {
            return Err(StoreError::NotFound(id));
        }
        let ordinal: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM sessions WHERE box = ?1",
            [id.get()],
            |row| row.get(0),
        )?;
        let state = SessionState::Starting;
        tx.execute(
            "INSERT INTO sessions (box, ordinal, state, started_at) VALUES (?1, ?2, ?3, ?4)",
            (id.get(), ordinal, state.as_str(), now.as_millis()),
        )?;
        let session = BoxSession {
            id: tx.last_insert_rowid(),
            ordinal,
            state,
        };
        tx.commit()?;
        drop(conn);
        Ok(session)
    }

    /// Every Session of Box `id`, by ordinal.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a stored state is unknown.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn box_sessions(&self, id: BoxId) -> Result<Vec<BoxSession>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn
            .prepare("SELECT id, ordinal, state FROM sessions WHERE box = ?1 ORDER BY ordinal")?;
        let rows = stmt.query_map([id.get()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, ordinal, state) = row?;
            Ok(BoxSession {
                id,
                ordinal,
                state: parsed_opt("session state", &state, SessionState::parse)?,
            })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests;
