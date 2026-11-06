//! Agent session rows and the last hook report cached per box.

use moor_core::config::AgentHolder;
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;

use moor_schema::tables::{Launches, Sessions};
use sea_query::{Expr, ExprTrait as _, Func, Order, Query};

use super::apply::insert_event;
use super::select::newest_session;
use super::{load_box, Store, StoreError};
use crate::store::db::{self, DbExt as _, OptionalExtension, Row};

/// The `launches` columns [`session_from_row`] reads, in order.
const SESSION_COLUMNS: [Launches; 11] = [
    Launches::Id,
    Launches::Box,
    Launches::SessionRef,
    Launches::Pane,
    Launches::StartedAt,
    Launches::EndedAt,
    Launches::Exit,
    Launches::TranscriptDigest,
    Launches::RegisteredAgentSession,
    Launches::Session,
    Launches::Holder,
];

/// A stored agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionRow {
    /// Row id.
    pub(crate) id: i64,
    /// Owning box.
    pub(crate) box_id: BoxId,
    /// The agent's own session reference.
    pub(crate) session_ref: String,
    /// The agent's pane, if it has one.
    pub(crate) pane: Option<String>,
    /// When the session began.
    pub(crate) started_at: Ts,
    /// When the session ended; none while live.
    pub(crate) ended_at: Option<Ts>,
    /// Exit code, if the agent exited.
    pub(crate) exit: Option<i32>,
    /// Digest of the saved transcript.
    pub(crate) transcript_digest: Option<String>,
    /// The agent session id last registered for resume against this session's pane.
    pub(crate) registered_agent_session: Option<String>,
    /// The Box's Session current when the agent started.
    pub(crate) session: Option<i64>,
    /// What holds the agent's terminal on the Box.
    pub(crate) holder: AgentHolder,
}

impl Store {
    /// The newest session of `box_id` that has not ended.
    pub(crate) fn live_session(&self, box_id: BoxId) -> Result<Option<SessionRow>, StoreError> {
        let conn = self.reader()?;
        Ok(conn
            .row(
                Query::select()
                    .columns(SESSION_COLUMNS)
                    .from(Launches::Table)
                    .and_where(Expr::col(Launches::Box).eq(box_id.get()))
                    .and_where(Expr::col(Launches::EndedAt).is_null())
                    .order_by(Launches::Id, Order::Desc)
                    .limit(1),
                session_from_row,
            )
            .optional()?)
    }

    /// When the first agent of `box_id` started, if one ever did.
    pub(crate) fn first_launch_at(&self, box_id: BoxId) -> Result<Option<Ts>, StoreError> {
        let conn = self.reader()?;
        let at: Option<i64> = conn.row(
            Query::select()
                .expr(Func::min(Expr::col(Launches::StartedAt)))
                .from(Launches::Table)
                .and_where(Expr::col(Launches::Box).eq(box_id.get())),
            |r| r.get(0),
        )?;
        Ok(at.map(Ts::from_millis))
    }

    /// What holds the terminal of the newest agent of `box_id`, ended or not.
    pub(crate) fn agent_holder(&self, box_id: BoxId) -> Result<AgentHolder, StoreError> {
        let conn = self.reader()?;
        let stored: Option<Option<String>> = conn
            .row(
                Query::select()
                    .column(Launches::Holder)
                    .from(Launches::Table)
                    .and_where(Expr::col(Launches::Box).eq(box_id.get()))
                    .order_by(Launches::Id, Order::Desc)
                    .limit(1),
                |r| r.get(0),
            )
            .optional()?;
        holder_from(stored.flatten())
            .map_err(|e| StoreError::Corrupt(format!("launches.holder: {e}")))
    }

    /// Forget the holders of `box_id`'s sessions, as moor did before it recorded them.
    #[cfg(test)]
    pub(crate) fn forget_holders(&self, box_id: BoxId) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::Holder, Option::<String>::None)
                .and_where(Expr::col(Launches::Box).eq(box_id.get())),
        )?;
        drop(conn);
        Ok(())
    }

    /// Whether `box_id` has ever had a session, ended or not.
    pub(crate) fn has_session(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let conn = self.reader()?;
        Ok(conn
            .row(
                Query::select()
                    .column(Launches::Id)
                    .from(Launches::Table)
                    .and_where(Expr::col(Launches::Box).eq(box_id.get()))
                    .limit(1),
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Record the start of a session on `box_id`, held by `holder`, and its `SessionStarted`
    /// event; box state is not touched.
    pub(crate) fn begin_session(
        &self,
        box_id: BoxId,
        session_ref: &str,
        pane: Option<&str>,
        holder: AgentHolder,
    ) -> Result<i64, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        if load_box(&tx, box_id)?.is_none() {
            return Err(StoreError::NotFound(box_id));
        }
        let current: Option<String> = tx
            .row(
                Query::select()
                    .column(Sessions::State)
                    .from(Sessions::Table)
                    .and_where(Expr::col(Sessions::Box).eq(box_id.get()))
                    .order_by(Sessions::Ordinal, Order::Desc)
                    .limit(1),
                |row| row.get(0),
            )
            .optional()?;
        if current.is_some_and(|state| state == "ended") {
            return Err(StoreError::Invalid(format!(
                "Box {} has no live Session to launch into",
                box_id.get()
            )));
        }
        let id: i64 = tx.row(
            Query::insert()
                .into_table(Launches::Table)
                .columns([
                    Launches::Box,
                    Launches::SessionRef,
                    Launches::Pane,
                    Launches::StartedAt,
                    Launches::Session,
                    Launches::Holder,
                ])
                .values_panic([
                    box_id.get().into(),
                    session_ref.into(),
                    pane.into(),
                    now.as_millis().into(),
                    db::sub(newest_session(box_id)),
                    holder.name().into(),
                ])
                .returning_col(Launches::Id),
            |r| r.get(0),
        )?;
        let started = Event::SessionStarted {
            session: session_ref.to_owned(),
        };
        insert_event(&tx, box_id, &started, now)?;
        tx.commit()?;
        drop(conn);
        Ok(id)
    }

    /// Record the end of a session; one that already ended is left untouched.
    pub(crate) fn end_session(
        &self,
        id: i64,
        exit: Option<i32>,
        transcript_digest: Option<&str>,
    ) -> Result<(), StoreError> {
        let now = self.now();
        let conn = self.writer();
        let mut q = Query::update();
        q.table(Launches::Table)
            .value(Launches::EndedAt, now.as_millis())
            .value(Launches::Exit, exit)
            .and_where(Expr::col(Launches::Id).eq(id))
            .and_where(Expr::col(Launches::EndedAt).is_null());
        if let Some(digest) = transcript_digest {
            q.value(Launches::TranscriptDigest, digest);
        }
        conn.run(&mut q)?;
        drop(conn);
        Ok(())
    }

    /// Keep the agent session id registered for resume against session `id`.
    pub(crate) fn set_registered_session(&self, id: i64, agent: &str) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::RegisteredAgentSession, agent)
                .and_where(Expr::col(Launches::Id).eq(id)),
        )?;
        drop(conn);
        Ok(())
    }

    /// Keep the digest of the transcript saved for a session that is still live.
    pub(crate) fn set_transcript_digest(&self, id: i64, digest: &str) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::TranscriptDigest, digest)
                .and_where(Expr::col(Launches::Id).eq(id))
                .and_where(Expr::col(Launches::EndedAt).is_null()),
        )?;
        drop(conn);
        Ok(())
    }
}

/// A session from a `launches` row.
fn session_from_row(r: &Row<'_>) -> db::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(0)?,
        box_id: BoxId::new(r.get(1)?),
        session_ref: r.get(2)?,
        pane: r.get(3)?,
        started_at: Ts::from_millis(r.get(4)?),
        ended_at: r.get::<_, Option<i64>>(5)?.map(Ts::from_millis),
        exit: r.get(6)?,
        transcript_digest: r.get(7)?,
        registered_agent_session: r.get(8)?,
        session: r.get(9)?,
        holder: holder_from(r.get(10)?).map_err(|e| db::Error::Convert {
            index: 10,
            source: e.into(),
        })?,
    })
}

/// The holder a `launches.holder` value names. A launch from before moor recorded the holder
/// has none, and was held by [`AgentHolder::UNRECORDED`].
fn holder_from(stored: Option<String>) -> Result<AgentHolder, String> {
    stored.map_or(Ok(AgentHolder::UNRECORDED), |name| {
        AgentHolder::from_name(&name).ok_or_else(|| format!("unknown holder {name:?}"))
    })
}

mod hook;

#[cfg(test)]
mod tests;
