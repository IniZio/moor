//! Sessions of a Box beyond the primary one inserted with it.

use moor_core::domain::event::Event;
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::BoxId;
use rusqlite::{OptionalExtension, TransactionBehavior};

use super::apply::insert_event;
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
    /// Returns [`StoreError::NotFound`] when the Box does not exist, [`StoreError::Invalid`] when its current Session is not Ended, or [`StoreError`] when the database fails.
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
        let current: Option<String> = tx
            .query_row(
                "SELECT state FROM sessions WHERE box = ?1 ORDER BY ordinal DESC LIMIT 1",
                [id.get()],
                |row| row.get(0),
            )
            .optional()?;
        if current.is_some_and(|state| state != SessionState::Ended.as_str()) {
            return Err(StoreError::Invalid(format!(
                "Box {} already has a live Session",
                id.get()
            )));
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

    /// Move a Starting Session to Working, the way a Box's first Session does once its agent runs.
    ///
    /// # Errors
    /// Returns [`StoreError::Illegal`] when the Session is not Starting, or [`StoreError`] when the database fails.
    pub(crate) fn start_session(&self, session: i64) -> Result<(), StoreError> {
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state: String = tx.query_row(
            "SELECT state FROM sessions WHERE id = ?1",
            [session],
            |row| row.get(0),
        )?;
        let to = parsed_opt("session state", &state, SessionState::parse)?
            .step(&SessionInput::Started)?;
        tx.execute(
            "UPDATE sessions SET state = ?1 WHERE id = ?2",
            (to.as_str(), session),
        )?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// End Box `id`'s current Session and its linked launch in one transaction.
    ///
    /// Returns `false`, writing nothing, when the Box has no Session or its current one already ended.
    /// A Ready Box with an Ended Session reads as Idle, so `idle_since` is stamped when unset.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] when the Box does not exist, [`StoreError::Illegal`] when the
    /// Session cannot end, or [`StoreError`] when the database fails.
    pub(crate) fn end_current_session(
        &self,
        id: BoxId,
        cause: &str,
        exit: Option<i32>,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists = tx
            .query_row("SELECT 1 FROM boxes WHERE id = ?1", [id.get()], |_| Ok(()))
            .optional()?;
        if exists.is_none() {
            return Err(StoreError::NotFound(id));
        }
        let current: Option<(i64, String)> = tx
            .query_row(
                "SELECT id, state FROM sessions WHERE box = ?1 ORDER BY ordinal DESC LIMIT 1",
                [id.get()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((session, state)) = current else {
            return Ok(false);
        };
        let state = parsed_opt("session state", &state, SessionState::parse)?;
        if state == SessionState::Ended {
            return Ok(false);
        }
        let ended = state.step(&SessionInput::End)?;
        let at = now.as_millis();
        tx.execute(
            "UPDATE sessions SET state = ?1, ended_at = ?2 WHERE id = ?3",
            (ended.as_str(), at, session),
        )?;
        tx.execute(
            "UPDATE launches SET ended_at = ?1, exit = ?2 WHERE session = ?3 AND ended_at IS NULL",
            (at, exit, session),
        )?;
        tx.execute(
            "UPDATE boxes SET idle_since = CASE WHEN state = 'ready' THEN COALESCE(idle_since, ?1) \
             ELSE idle_since END, updated_at = ?1 WHERE id = ?2",
            (at, id.get()),
        )?;
        let event = Event::SessionEnded {
            session,
            cause: cause.to_owned(),
            exit,
        };
        insert_event(&tx, id, &event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(true)
    }

    /// Every Session of Box `id`, by ordinal.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a stored state is unknown.
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
