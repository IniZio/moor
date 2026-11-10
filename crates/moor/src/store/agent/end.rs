//! Ending a launch, and refusing writes made from a reading of one that is no longer live.

use moor_core::domain::decision::Effect;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, LaunchId};
use moor_schema::tables::{BoxState, Launches};
use sea_query::{Expr, ExprTrait as _, Order, Query, SelectStatement, Value};

use crate::store::db::{Db, DbExt as _, OptionalExtension, Tx};
use crate::store::select::state_update;
use crate::store::sessions::end_current;
use crate::store::{Fence, Store, StoreError};

/// How a launch ended.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LaunchEnd<'a> {
    /// The agent's exit code, when it left one.
    pub(crate) exit: Option<i32>,
    /// The digest of the transcript saved as it ended.
    pub(crate) digest: Option<&'a str>,
    /// Why, when the Box's current Session ends with the launch.
    pub(crate) session: Option<&'a str>,
}

impl Store {
    /// Ends launch `id` of Box `box_id`, and with `end.session` the Box's current Session too, in
    /// one transaction.
    ///
    /// # Errors
    /// [`StoreError::Stale`] when `id` is not the Box's live launch: it ended, or a newer one
    /// began. [`StoreError::Illegal`] when the Session cannot end, or [`StoreError`] when the
    /// database fails.
    pub(crate) fn end_launch(
        &self,
        box_id: BoxId,
        id: LaunchId,
        end: &LaunchEnd<'_>,
    ) -> Result<(), StoreError> {
        self.check_write(box_id);
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        fence_launch(&tx, box_id, id)?;
        let mut q = Query::update();
        q.table(Launches::Table)
            .value(Launches::EndedAt, now.as_millis())
            .value(Launches::Exit, end.exit)
            .and_where(Expr::col(Launches::Id).eq(id.get()))
            .and_where(Expr::col(Launches::EndedAt).is_null());
        if let Some(digest) = end.digest {
            q.value(Launches::TranscriptDigest, digest);
        }
        tx.run(&mut q)?;
        if let Some(cause) = end.session {
            end_current(&tx, box_id, (cause, end.exit), now)?;
        }
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// [`Store::end_launch`] of launch `id`, alone, read from its row.
    #[cfg(test)]
    pub(crate) fn end_launch_with(
        &self,
        id: LaunchId,
        exit: Option<i32>,
        digest: Option<&str>,
    ) -> Result<(), StoreError> {
        let box_id: i64 = self.reader()?.row(
            Query::select()
                .column(Launches::Box)
                .from(Launches::Table)
                .and_where(Expr::col(Launches::Id).eq(id.get())),
            |r| r.get(0),
        )?;
        let end = LaunchEnd {
            exit,
            digest,
            session: None,
        };
        self.end_launch(BoxId::new(box_id), id, &end)
    }
}

/// The query for Box `box_id`'s live launch, the newest that has not ended, without columns.
pub(super) fn live(box_id: BoxId) -> SelectStatement {
    Query::select()
        .from(Launches::Table)
        .and_where(Expr::col(Launches::Box).eq(box_id.get()))
        .and_where(Expr::col(Launches::EndedAt).is_null())
        .order_by(Launches::Id, Order::Desc)
        .limit(1)
        .take()
}

/// Refuses a write made from a reading of launch `read` once it is not Box `box_id`'s live launch.
pub(in crate::store) fn fence_launch(
    conn: &dyn Db,
    box_id: BoxId,
    read: LaunchId,
) -> Result<(), StoreError> {
    let live: Option<i64> = conn
        .row(live(box_id).column(Launches::Id), |r| r.get(0))
        .optional()?;
    if live == Some(read.get()) {
        return Ok(());
    }
    Err(StoreError::Stale {
        box_id,
        read: Fence::Launch(read),
    })
}

/// Ends Box `box_id`'s live launches at `now`: their agent ran on hands the Box no longer has.
/// The Session stays open, since its conversation carries on in the new hands.
pub(in crate::store) fn end_live_launches(
    tx: &Tx<'_>,
    box_id: BoxId,
    now: Ts,
) -> Result<(), StoreError> {
    tx.run(
        Query::update()
            .table(Launches::Table)
            .value(Launches::EndedAt, now.as_millis())
            .and_where(Expr::col(Launches::Box).eq(box_id.get()))
            .and_where(Expr::col(Launches::EndedAt).is_null()),
    )?;
    Ok(())
}

/// Ends the launch in flight on `box_id` inside the caller's write. Any other effect is left, so
/// a run that began meanwhile keeps its own.
pub(super) fn end_launching(tx: &dyn Db, box_id: BoxId, now: Ts) -> Result<(), StoreError> {
    tx.run(
        state_update(box_id)
            .value(BoxState::Effect, Value::String(None))
            .value(BoxState::EffectAttempt, Value::BigInt(None))
            .value(BoxState::EffectRun, Value::BigInt(None))
            .value(BoxState::EffectAt, Value::BigInt(None))
            .value(BoxState::UpdatedAt, now.as_millis())
            .and_where(Expr::col(BoxState::Effect).eq(Effect::Launch.as_str())),
    )?;
    Ok(())
}
