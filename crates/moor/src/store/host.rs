//! Sessions that run on the host, with no Box.

use moor_core::domain::session_state::{SessionInput, SessionState};
use rusqlite::TransactionBehavior;

use super::{Store, StoreError};

impl Store {
    /// Register a Session that runs on the host in `workdir`, with no Box; returns its id.
    ///
    /// The Session is Working at once and records no events, since events belong to a Box.
    ///
    /// # Errors
    /// Returns [`StoreError::Invalid`] when `workdir` is empty, [`StoreError::Illegal`] when the
    /// Session cannot start, or [`StoreError`] when the database fails.
    pub(crate) fn open_host_session(
        &self,
        workdir: &str,
        agent: Option<&str>,
        agent_session_id: Option<&str>,
    ) -> Result<i64, StoreError> {
        if workdir.is_empty() {
            return Err(StoreError::Invalid(
                "a host Session needs a workdir".to_owned(),
            ));
        }
        let state = SessionState::initial(&SessionInput::Open)?.step(&SessionInput::Started)?;
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO sessions (box, ordinal, state, started_at, workdir, agent, agent_session_id) \
             VALUES (NULL, 1, ?1, ?2, ?3, ?4, ?5)",
            (state.as_str(), now.as_millis(), workdir, agent, agent_session_id),
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        drop(conn);
        Ok(id)
    }
}

#[cfg(test)]
mod tests;
