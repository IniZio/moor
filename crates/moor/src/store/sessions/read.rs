//! Reading one Session, and the child Sessions of a parent, with the Box each belongs to.

use moor_core::domain::session_state::SessionState;
use moor_core::BoxName;
use rusqlite::{OptionalExtension, Row};

use super::super::{parsed, parsed_opt, Store, StoreError};

/// One Session, with the name of its Box when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionRecord {
    /// Row id of the Session.
    pub(crate) id: i64,
    /// Name of the Box the Session runs in; `None` for a host Session.
    pub(crate) task: Option<BoxName>,
    /// Position among the Box's Sessions, starting at 1.
    pub(crate) ordinal: i64,
    /// Row id of the parent Session, when it has one.
    pub(crate) parent: Option<i64>,
    /// Working directory of a host Session.
    pub(crate) workdir: Option<String>,
    /// Agent the Session runs.
    pub(crate) agent: Option<String>,
    /// The agent's own id for the Session.
    pub(crate) agent_session_id: Option<String>,
    /// Where the Session is in its life.
    pub(crate) state: SessionState,
    /// When the Session started, in milliseconds.
    pub(crate) started_at: i64,
    /// When the Session ended, in milliseconds.
    pub(crate) ended_at: Option<i64>,
}

/// Columns of a Session row, in the order [`SessionRecord::from_row`] reads them.
const SELECT: &str = "SELECT s.id, b.name, s.ordinal, s.parent_session, s.workdir, s.agent, \
     s.agent_session_id, s.state, s.started_at, s.ended_at \
     FROM sessions s LEFT JOIN boxes b ON b.id = s.box";

impl SessionRecord {
    /// Whether the Session has ended.
    pub(crate) fn is_ended(&self) -> bool {
        self.state == SessionState::Ended || self.ended_at.is_some()
    }

    /// Read a row of [`SELECT`], parsing its Box name and state.
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Result<Self, StoreError>> {
        let name: Option<String> = row.get(1)?;
        let state: String = row.get(7)?;
        let parsed_row = (|| {
            Ok(Self {
                id: row.get(0)?,
                task: name
                    .map(|n| parsed("session task", &n, BoxName::parse_allow_bare))
                    .transpose()?,
                ordinal: row.get(2)?,
                parent: row.get(3)?,
                workdir: row.get(4)?,
                agent: row.get(5)?,
                agent_session_id: row.get(6)?,
                state: parsed_opt("session state", &state, SessionState::parse)?,
                started_at: row.get(8)?,
                ended_at: row.get(9)?,
            })
        })();
        Ok(parsed_row)
    }
}

impl Store {
    /// Session `id`, or `None` when there is none.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a stored name or state is unknown.
    pub(crate) fn session_row(&self, id: i64) -> Result<Option<SessionRecord>, StoreError> {
        let conn = self.reader()?;
        conn.query_row(
            &format!("{SELECT} WHERE s.id = ?1"),
            [id],
            SessionRecord::from_row,
        )
        .optional()?
        .transpose()
    }

    /// Sessions that have not ended, oldest first (by id).
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a stored name or state is unknown.
    pub(crate) fn live_sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "{SELECT} WHERE s.state <> ?1 AND s.ended_at IS NULL ORDER BY s.id"
        ))?;
        let rows = stmt.query_map([SessionState::Ended.as_str()], SessionRecord::from_row)?;
        rows.map(|row| row?).collect()
    }

    /// Sessions whose parent is `parent`, oldest first (by id).
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a stored name or state is unknown.
    pub(crate) fn session_children(&self, parent: i64) -> Result<Vec<SessionRecord>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "{SELECT} WHERE s.parent_session = ?1 ORDER BY s.id"
        ))?;
        let rows = stmt.query_map([parent], SessionRecord::from_row)?;
        rows.map(|row| row?).collect()
    }
}

#[cfg(test)]
mod tests;
