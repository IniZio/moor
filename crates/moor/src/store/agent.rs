//! Agent launch rows.

use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, LaunchId};

use moor_schema::tables::{Launches, Sessions};
use sea_query::{Expr, ExprTrait as _, Func, Order, Query};

use super::apply::insert_event;
use super::select::newest_session;
use super::{load_box, Store, StoreError};
use crate::store::db::{self, DbExt as _, OptionalExtension, Row};

/// The `launches` columns [`launch_from_row`] reads, in order.
const LAUNCH_COLUMNS: [Launches; 9] = [
    Launches::Id,
    Launches::Box,
    Launches::SessionRef,
    Launches::StartedAt,
    Launches::EndedAt,
    Launches::Exit,
    Launches::TranscriptDigest,
    Launches::RegisteredAgentSession,
    Launches::Session,
];

/// The `launches.holder` value of every agent moor starts: herdr-core.
const HOLDER: &str = "hc";

/// One start of a Box's agent, a `launches` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    /// Row id.
    pub(crate) id: LaunchId,
    /// Owning box.
    pub(crate) box_id: BoxId,
    /// The agent's own session reference.
    pub(crate) session_ref: String,
    /// When the launch began.
    pub(crate) started_at: Ts,
    /// When the launch ended; none while live.
    pub(crate) ended_at: Option<Ts>,
    /// Exit code, if the agent exited.
    pub(crate) exit: Option<i32>,
    /// Digest of the saved transcript.
    pub(crate) transcript_digest: Option<String>,
    /// The agent session id last registered for resume against this launch's pane.
    pub(crate) registered_agent_session: Option<String>,
    /// The Box's Session current when the agent started.
    pub(crate) session: Option<i64>,
}

impl Store {
    /// The newest launch of `box_id` that has not ended.
    pub(crate) fn live_launch(&self, box_id: BoxId) -> Result<Option<Launch>, StoreError> {
        let conn = self.reader()?;
        Ok(conn
            .row(end::live(box_id).columns(LAUNCH_COLUMNS), launch_from_row)
            .optional()?)
    }

    /// Launch `id`, ended or not.
    pub(crate) fn launch(&self, id: LaunchId) -> Result<Option<Launch>, StoreError> {
        let conn = self.reader()?;
        Ok(conn
            .row(
                Query::select()
                    .columns(LAUNCH_COLUMNS)
                    .from(Launches::Table)
                    .and_where(Expr::col(Launches::Id).eq(id.get())),
                launch_from_row,
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

    /// Refuses a Box whose newest agent was held by anything but herdr-core, as every launch
    /// before moor held agents with herdr-core alone was; such a Box is closed and recreated.
    pub(crate) fn require_hc_holder(&self, box_id: BoxId, name: &str) -> Result<(), StoreError> {
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
        match stored {
            None => Ok(()),
            Some(Some(held)) if held == HOLDER => Ok(()),
            Some(_) => Err(StoreError::Invalid(format!(
                "the agent of Box {name} was not started by this version of moor and cannot be \
                 reached; close the Box and recreate it"
            ))),
        }
    }

    /// Record `holder` as the holder of `box_id`'s sessions, as another moor did.
    #[cfg(test)]
    pub(crate) fn set_holders(
        &self,
        box_id: BoxId,
        holder: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::Holder, holder.map(str::to_owned))
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

    /// Record the start of a launch on `box_id` and its `SessionStarted` event; box state is
    /// not touched.
    pub(crate) fn begin_launch(
        &self,
        box_id: BoxId,
        session_ref: &str,
    ) -> Result<LaunchId, StoreError> {
        self.begin_launch_with(box_id, session_ref, None)
    }

    /// [`Store::begin_launch`] for an agent launched on `argv` as its first prompt: the oldest
    /// queued prompt row holding that text is sent to the new launch in the same transaction.
    pub(crate) fn begin_launch_with(
        &self,
        box_id: BoxId,
        session_ref: &str,
        argv: Option<&str>,
    ) -> Result<LaunchId, StoreError> {
        self.check_write(box_id);
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
        let id = LaunchId::new(
            tx.row(
                Query::insert()
                    .into_table(Launches::Table)
                    .columns([
                        Launches::Box,
                        Launches::SessionRef,
                        Launches::StartedAt,
                        Launches::Session,
                        Launches::Holder,
                    ])
                    .values_panic([
                        box_id.get().into(),
                        session_ref.into(),
                        now.as_millis().into(),
                        db::sub(newest_session(box_id)),
                        HOLDER.into(),
                    ])
                    .returning_col(Launches::Id),
                |r| r.get(0),
            )?,
        );
        let started = Event::SessionStarted {
            session: session_ref.to_owned(),
        };
        insert_event(&tx, box_id, &started, now)?;
        if let Some(text) = argv {
            super::inbox::send_with_launch(&tx, box_id, (id, session_ref), text, now)?;
        }
        tx.commit()?;
        drop(conn);
        Ok(id)
    }

    /// Keep the agent session id registered for resume against launch `id`.
    pub(crate) fn set_registered_session(
        &self,
        id: LaunchId,
        agent: &str,
    ) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::RegisteredAgentSession, agent)
                .and_where(Expr::col(Launches::Id).eq(id.get())),
        )?;
        drop(conn);
        Ok(())
    }

    /// Keep the digest of the transcript saved for a launch that is still live.
    pub(crate) fn set_transcript_digest(
        &self,
        id: LaunchId,
        digest: &str,
    ) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::TranscriptDigest, digest)
                .and_where(Expr::col(Launches::Id).eq(id.get()))
                .and_where(Expr::col(Launches::EndedAt).is_null()),
        )?;
        drop(conn);
        Ok(())
    }

    /// Keep that the agent of live launch `id` answered a read at `at`.
    pub(crate) fn note_agent_read(&self, id: LaunchId, at: Ts) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::AgentReadAt, at.as_millis())
                .and_where(Expr::col(Launches::Id).eq(id.get()))
                .and_where(Expr::col(Launches::EndedAt).is_null()),
        )?;
        drop(conn);
        Ok(())
    }

    /// Keep that a `needs_input` was written for the status of live launch `id` set at `at`, on
    /// the Box's clock; whether one was written for it already.
    pub(crate) fn note_asked(&self, id: LaunchId, at: Ts) -> Result<bool, StoreError> {
        let conn = self.writer();
        let changed = conn.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::AskedAt, at.as_millis())
                .and_where(Expr::col(Launches::Id).eq(id.get()))
                .and_where(
                    Expr::col(Launches::AskedAt)
                        .is_null()
                        .or(Expr::col(Launches::AskedAt).ne(at.as_millis())),
                ),
        )?;
        drop(conn);
        Ok(changed == 0)
    }

    /// Keep that an `agent_said` was written for the idle status of launch `id` set at `at`, on
    /// the Box's clock; whether one was written for it already.
    pub(crate) fn note_said(&self, id: LaunchId, at: Ts) -> Result<bool, StoreError> {
        let conn = self.writer();
        let changed = conn.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::SaidAt, at.as_millis())
                .and_where(Expr::col(Launches::Id).eq(id.get()))
                .and_where(
                    Expr::col(Launches::SaidAt)
                        .is_null()
                        .or(Expr::col(Launches::SaidAt).ne(at.as_millis())),
                ),
        )?;
        drop(conn);
        Ok(changed == 0)
    }

    /// Whether the live agent of `box_id` has answered a read, but none for so long that it may
    /// have started a turn unseen.
    pub(crate) fn agent_unread(&self, box_id: BoxId, now: Ts) -> Result<bool, StoreError> {
        let Some(live) = self.live_launch(box_id)? else {
            return Ok(false);
        };
        let conn = self.reader()?;
        let at: Option<i64> = conn.row(
            Query::select()
                .column(Launches::AgentReadAt)
                .from(Launches::Table)
                .and_where(Expr::col(Launches::Id).eq(live.id.get())),
            |r| r.get(0),
        )?;
        let ttl = i64::try_from(AGENT_READ_TTL.as_millis()).unwrap_or(i64::MAX);
        Ok(at.is_some_and(|at| now.as_millis().saturating_sub(at) >= ttl))
    }
}

/// How long a read of an agent stands for a dropped one.
const AGENT_READ_TTL: std::time::Duration = std::time::Duration::from_secs(120);

/// A launch from its `launches` row.
fn launch_from_row(r: &Row<'_>) -> db::Result<Launch> {
    Ok(Launch {
        id: LaunchId::new(r.get(0)?),
        box_id: BoxId::new(r.get(1)?),
        session_ref: r.get(2)?,
        started_at: Ts::from_millis(r.get(3)?),
        ended_at: r.get::<_, Option<i64>>(4)?.map(Ts::from_millis),
        exit: r.get(5)?,
        transcript_digest: r.get(6)?,
        registered_agent_session: r.get(7)?,
        session: r.get(8)?,
    })
}

mod end;

pub(crate) use end::LaunchEnd;
pub(super) use end::{end_live_launches, fence_launch};

#[cfg(test)]
mod tests;
