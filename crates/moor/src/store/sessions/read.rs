//! Reading one Session, and the child Sessions of a parent, with the Box each belongs to.

use moor_core::domain::session_state::SessionState;
use moor_core::user::UserId;
use moor_core::{BoxId, BoxName};
use moor_schema::tables::{Boxes, Sessions};
use sea_query::{Expr, ExprTrait as _, Order, Query, SelectStatement};

use super::super::{parsed, parsed_opt, Store, StoreError};
use crate::store::db::{DbExt as _, OptionalExtension, Row};

/// One Session, with the name of its Box when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionRecord {
    /// Row id of the Session.
    pub(crate) id: i64,
    /// Name of the Box the Session runs in; `None` for a host Session.
    pub(crate) box_name: Option<BoxName>,
    /// Id of that Box.
    pub(crate) box_id: Option<BoxId>,
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
    /// The user a host Session belongs to; `None` for a Box's Session and the operator's.
    pub(crate) user: Option<UserId>,
    /// The key a host Session was opened with.
    pub(crate) key: Option<String>,
}

/// The Session rows, with columns in the order [`SessionRecord::from_row`] reads them.
fn select() -> SelectStatement {
    let s = |c| (Sessions::Table, c);
    Query::select()
        .column(s(Sessions::Id))
        .column((Boxes::Table, Boxes::Name))
        .columns(
            [
                Sessions::Ordinal,
                Sessions::ParentSession,
                Sessions::Workdir,
                Sessions::Agent,
                Sessions::AgentSessionId,
                Sessions::State,
                Sessions::StartedAt,
                Sessions::EndedAt,
                Sessions::Box,
                Sessions::User,
                Sessions::HostKey,
            ]
            .map(s),
        )
        .from(Sessions::Table)
        .left_join(
            Boxes::Table,
            Expr::col((Boxes::Table, Boxes::Id)).equals(s(Sessions::Box)),
        )
        .take()
}

impl SessionRecord {
    /// Whether the Session has ended.
    pub(crate) fn is_ended(&self) -> bool {
        self.state == SessionState::Ended || self.ended_at.is_some()
    }

    /// Read a row of [`select`], parsing its Box name and state.
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Result<Self, StoreError>> {
        let name: Option<String> = row.get(1)?;
        let state: String = row.get(7)?;
        let parsed_row = (|| {
            Ok(Self {
                id: row.get(0)?,
                box_name: name
                    .map(|n| parsed("session box name", &n, BoxName::parse_allow_bare))
                    .transpose()?,
                ordinal: row.get(2)?,
                parent: row.get(3)?,
                workdir: row.get(4)?,
                agent: row.get(5)?,
                agent_session_id: row.get(6)?,
                state: parsed_opt("session state", &state, SessionState::parse)?,
                started_at: row.get(8)?,
                ended_at: row.get(9)?,
                box_id: row.get::<_, Option<i64>>(10)?.map(BoxId::new),
                user: row.get::<_, Option<i64>>(11)?.map(UserId::new),
                key: row.get(12)?,
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
        conn.row(
            select().and_where(Expr::col((Sessions::Table, Sessions::Id)).eq(id)),
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
        let rows = conn.rows(
            select()
                .and_where(
                    Expr::col((Sessions::Table, Sessions::State)).ne(SessionState::Ended.as_str()),
                )
                .and_where(Expr::col((Sessions::Table, Sessions::EndedAt)).is_null())
                .order_by((Sessions::Table, Sessions::Id), Order::Asc),
            SessionRecord::from_row,
        )?;
        rows.map(|row| row?).collect()
    }

    /// Sessions whose parent is `parent`, oldest first (by id).
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a stored name or state is unknown.
    pub(crate) fn session_children(&self, parent: i64) -> Result<Vec<SessionRecord>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            select()
                .and_where(Expr::col((Sessions::Table, Sessions::ParentSession)).eq(parent))
                .order_by((Sessions::Table, Sessions::Id), Order::Asc),
            SessionRecord::from_row,
        )?;
        rows.map(|row| row?).collect()
    }
}

#[cfg(test)]
mod tests;
