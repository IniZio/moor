//! Sessions that run on the host, with no Box.

use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::user::UserId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::{parsed_opt, Store, StoreError};

/// One host Session row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostRow {
    /// Session id.
    pub(crate) id: i64,
    /// Directory the agent runs in.
    pub(crate) workdir: String,
    /// Agent name, when known.
    pub(crate) agent: Option<String>,
    /// The agent's own session id, when known.
    pub(crate) agent_session_id: Option<String>,
    /// Current state.
    pub(crate) state: SessionState,
    /// When the Session ended, in milliseconds.
    pub(crate) ended_at: Option<i64>,
}

/// Read the host Session `id`; Box Sessions are not host rows.
fn load(conn: &Connection, id: i64) -> Result<Option<HostRow>, StoreError> {
    let raw = conn
        .query_row(
            "SELECT workdir, agent, agent_session_id, state, ended_at FROM sessions \
             WHERE id = ?1 AND box IS NULL",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((workdir, agent, agent_session_id, state, ended_at)) = raw else {
        return Ok(None);
    };
    let state = parsed_opt("session state", &state, SessionState::parse)?;
    Ok(Some(HostRow {
        id,
        workdir,
        agent,
        agent_session_id,
        state,
        ended_at,
    }))
}

/// Read a host Session that still runs, or say why it cannot change.
fn load_live(conn: &Connection, id: i64) -> Result<HostRow, StoreError> {
    let row =
        load(conn, id)?.ok_or_else(|| StoreError::Invalid(format!("no host Session {id}")))?;
    if row.ended_at.is_some() || row.state == SessionState::Ended {
        return Err(StoreError::Invalid(format!("host Session {id} has ended")));
    }
    Ok(row)
}

impl Store {
    /// Register a Session that runs on the host in `workdir`, with no parent; returns its id.
    ///
    /// # Errors
    /// As [`Store::open_child_host_session`].
    #[cfg(test)]
    pub(crate) fn open_host_session(
        &self,
        workdir: &str,
        agent: Option<&str>,
        agent_session_id: Option<&str>,
    ) -> Result<i64, StoreError> {
        self.open_child_host_session(workdir, agent, agent_session_id, None, None)
    }

    /// Register a Session that runs on the host in `workdir`, with no Box, under `parent` when
    /// given; returns its id.
    ///
    /// The Session is Working at once and records no events, since events belong to a Box. The
    /// parent is checked in the same transaction as the insert.
    ///
    /// # Errors
    /// Returns [`StoreError::Invalid`] when `workdir` is empty or `parent` is unknown or has
    /// ended, [`StoreError::Illegal`] when the Session cannot start, or [`StoreError`] when the
    /// database fails.
    pub(crate) fn open_child_host_session(
        &self,
        workdir: &str,
        agent: Option<&str>,
        agent_session_id: Option<&str>,
        parent: Option<i64>,
        user: Option<UserId>,
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
        if let Some(p) = parent {
            let row: Option<(String, Option<i64>)> = tx
                .query_row(
                    "SELECT state, ended_at FROM sessions WHERE id = ?1",
                    [p],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match row {
                None => return Err(StoreError::Invalid(format!("no Session {p}"))),
                Some((s, ended_at)) if s == SessionState::Ended.as_str() || ended_at.is_some() => {
                    return Err(StoreError::Invalid(format!("Session {p} has ended")));
                }
                Some(_) => {}
            }
        }
        tx.execute(
            "INSERT INTO sessions (box, ordinal, state, started_at, workdir, agent, agent_session_id, parent_session, user) \
             VALUES (NULL, 1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (state.as_str(), now.as_millis(), workdir, agent, agent_session_id, parent, user.map(UserId::get)),
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        drop(conn);
        Ok(id)
    }

    /// Read the host Session `id`; `None` when no host Session has that id.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or the stored state is unknown.
    #[cfg(test)]
    pub(crate) fn host_session(&self, id: i64) -> Result<Option<HostRow>, StoreError> {
        load(&self.reader()?, id)
    }

    /// Set the agent fields that are `Some` on a live host Session; `None` leaves a column as is.
    ///
    /// # Errors
    /// Returns [`StoreError::Invalid`] when no host Session has that id or it has ended, or
    /// [`StoreError`] when the database fails.
    pub(crate) fn update_host_session(
        &self,
        id: i64,
        agent: Option<&str>,
        agent_session_id: Option<&str>,
    ) -> Result<HostRow, StoreError> {
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        load_live(&tx, id)?;
        tx.execute(
            "UPDATE sessions SET agent = COALESCE(?2, agent), \
             agent_session_id = COALESCE(?3, agent_session_id) WHERE id = ?1",
            (id, agent, agent_session_id),
        )?;
        let row = load_live(&tx, id)?;
        tx.commit()?;
        drop(conn);
        Ok(row)
    }

    /// End a live host Session, recording when; records no events.
    ///
    /// # Errors
    /// Returns [`StoreError::Invalid`] when no host Session has that id or it has ended,
    /// [`StoreError::Illegal`] when its state cannot end, or [`StoreError`] when the database fails.
    pub(crate) fn end_host_session(&self, id: i64) -> Result<HostRow, StoreError> {
        let now = self.now().as_millis();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut row = load_live(&tx, id)?;
        row.state = row.state.step(&SessionInput::End)?;
        tx.execute(
            "UPDATE sessions SET state = ?2, ended_at = ?3 WHERE id = ?1",
            (id, row.state.as_str(), now),
        )?;
        tx.commit()?;
        drop(conn);
        row.ended_at = Some(now);
        Ok(row)
    }
}

#[cfg(test)]
mod tests;
