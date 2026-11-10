//! Reads of what a Box's launches, row and Proxy say about its agent: how often moor relaunched
//! it, what it still owes the agent, and when its tunnel last restarted.

use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::{BoxState, Launches, Proxy};
use sea_query::{Expr, ExprTrait as _, Query, Value};

use super::db::{DbExt as _, OptionalExtension};
use super::select::state_update;
use super::{Store, StoreError};

/// The prefix of a note owed for a suspension, as `box_state.owed_note` holds it.
const SUSPENDED: &str = r#"{"kind":"suspended""#;

/// A note an agent is owed once, about what happened to its Box while it was away.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum OwedNote {
    /// The Box's machine was replaced, with a new disk.
    DiskLost,
    /// The Box stopped mid-turn at `at`, unix seconds; `stopped` when its machine stopped under
    /// it rather than moor suspending it.
    Suspended {
        /// When, in unix seconds.
        at: i64,
        /// Whether the machine stopped on its own.
        stopped: bool,
    },
}

impl Store {
    /// How many of Box `id`'s launches since `since` were relaunches moor made on its own.
    pub(crate) fn caused_launches_since(&self, id: BoxId, since: Ts) -> Result<usize, StoreError> {
        let n: i64 = self.reader()?.row(
            Query::select()
                .expr(Expr::col(Launches::Id).count())
                .from(Launches::Table)
                .and_where(Expr::col(Launches::Box).eq(id.get()))
                .and_where(Expr::col(Launches::Cause).is_not_null())
                .and_where(Expr::col(Launches::StartedAt).gt(since.as_millis())),
            |r| r.get(0),
        )?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// When moor last relaunched the agent of Box `id` itself.
    pub(crate) fn last_caused_launch(&self, id: BoxId) -> Result<Option<Ts>, StoreError> {
        let at: Option<i64> = self.reader()?.row(
            Query::select()
                .expr(Expr::col(Launches::StartedAt).max())
                .from(Launches::Table)
                .and_where(Expr::col(Launches::Box).eq(id.get()))
                .and_where(Expr::col(Launches::Cause).is_not_null()),
            |r| r.get(0),
        )?;
        Ok(at.map(Ts::from_millis))
    }

    /// When the relaunch cap was recorded for Box `id` since its last relaunch.
    pub(crate) fn relaunch_capped_at(&self, id: BoxId) -> Result<Option<Ts>, StoreError> {
        let at: Option<Option<i64>> = self
            .reader()?
            .row(
                Query::select()
                    .column(BoxState::RelaunchCappedAt)
                    .from(BoxState::Table)
                    .and_where(Expr::col(BoxState::Box).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(at.flatten().map(Ts::from_millis))
    }

    /// When the tunnel of Box `id` last restarted, as its keeper recorded.
    pub(crate) fn tunnel_restarted_at(&self, id: BoxId) -> Result<Option<Ts>, StoreError> {
        let at: Option<Option<i64>> = self
            .reader()?
            .row(
                Query::select()
                    .column(Proxy::RestartedAt)
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::Box).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(at.flatten().map(Ts::from_millis))
    }

    /// The note Box `id`'s agent is owed, if any.
    pub(crate) fn owed_note(&self, id: BoxId) -> Result<Option<OwedNote>, StoreError> {
        let text: Option<Option<String>> = self
            .reader()?
            .row(
                Query::select()
                    .column(BoxState::OwedNote)
                    .from(BoxState::Table)
                    .and_where(Expr::col(BoxState::Box).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(text
            .flatten()
            .map(|t| serde_json::from_str(&t))
            .transpose()?)
    }

    /// Marks Box `id` to resume first and owes its agent a note that it stopped at `at`, unix
    /// seconds; `stopped` when its machine stopped under it.
    pub(crate) fn owe_suspended(
        &self,
        id: BoxId,
        at: i64,
        stopped: bool,
    ) -> Result<(), StoreError> {
        let note = serde_json::to_string(&OwedNote::Suspended { at, stopped })?;
        self.writer().run(
            state_update(id)
                .value(BoxState::ResumeFirstAt, at)
                .value(BoxState::OwedNote, note),
        )?;
        Ok(())
    }

    /// Drops the note Box `id`'s agent was owed, once told or no longer true.
    pub(crate) fn drop_owed_note(&self, id: BoxId) -> Result<(), StoreError> {
        self.writer()
            .run(state_update(id).value(BoxState::OwedNote, Value::String(None)))?;
        Ok(())
    }

    /// Drops the note owed for a suspension of Box `id`, keeping any other.
    pub(super) fn drop_suspended_note(&self, id: BoxId) -> Result<(), StoreError> {
        self.writer().run(
            state_update(id)
                .value(BoxState::OwedNote, Value::String(None))
                .and_where(Expr::col(BoxState::OwedNote).like(format!("{SUSPENDED}%"))),
        )?;
        Ok(())
    }
}
