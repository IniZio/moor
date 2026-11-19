//! When a launch's agent last answered a read.

use moor_core::seam::clock::Ts;
use moor_core::{BoxId, LaunchId};
use moor_schema::tables::Launches;
use sea_query::{Expr, ExprTrait as _, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// How long a read of an agent stands for a dropped one.
const AGENT_READ_TTL: std::time::Duration = std::time::Duration::from_secs(120);

impl Store {
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

    /// When the agent of launch `id` last answered a read; none when it never has.
    pub(crate) fn agent_read_at(&self, id: LaunchId) -> Result<Option<Ts>, StoreError> {
        let conn = self.reader()?;
        let at: Option<Option<i64>> = conn
            .row(
                Query::select()
                    .column(Launches::AgentReadAt)
                    .from(Launches::Table)
                    .and_where(Expr::col(Launches::Id).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(at.flatten().map(Ts::from_millis))
    }

    /// Whether the live agent of `box_id` has answered a read, but none for so long that it may
    /// have started a turn unseen.
    pub(crate) fn agent_unread(&self, box_id: BoxId, now: Ts) -> Result<bool, StoreError> {
        let Some(live) = self.live_launch(box_id)? else {
            return Ok(false);
        };
        let ttl = i64::try_from(AGENT_READ_TTL.as_millis()).unwrap_or(i64::MAX);
        Ok(self
            .agent_read_at(live.id)?
            .is_some_and(|at| now.as_millis().saturating_sub(at.as_millis()) >= ttl))
    }
}
