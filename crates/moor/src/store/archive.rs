//! Where each Session's newest archive is, and when a Box owes one.
//!
//! The pointer only moves forward on (hands inc, taken at), so a save that finishes late on
//! hands the Box has since replaced cannot point the Session back at an older conversation.

use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::{BoxState, Sessions};
use sea_query::{Cond, Expr, ExprTrait as _, Query, Value};

use super::{Store, StoreError};
use crate::store::db::{Db, DbExt as _, OptionalExtension as _};

/// An archive of the agent's project directory, as a Session points at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchivePointer {
    /// The archive's blob key.
    pub(crate) key: String,
    /// The Box's hands inc that took the archive.
    pub(crate) inc: i64,
    /// When the archive was taken.
    pub(crate) at: Ts,
}

impl Store {
    /// When Box `id`'s archive falls due; none when no save is owed.
    pub(crate) fn archive_due(&self, id: BoxId) -> Result<Option<Ts>, StoreError> {
        let conn = self.reader()?;
        let due: Option<Option<i64>> = conn
            .row(
                Query::select()
                    .column(BoxState::ArchiveDue)
                    .from(BoxState::Table)
                    .and_where(Expr::col(BoxState::Box).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(due.flatten().map(Ts::from_millis))
    }

    /// Points Session `session` of Box `box_id` at `to`, unless it points at an archive taken by
    /// newer hands, or by the same hands later. When it moves, an archive due no later than
    /// `to.at` is paid. Returns whether it moved.
    pub(crate) fn advance_archive(
        &self,
        box_id: BoxId,
        session: i64,
        to: &ArchivePointer,
    ) -> Result<bool, StoreError> {
        self.check_write(box_id);
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let behind = Cond::any()
            .add(Expr::col(Sessions::ArchiveInc).is_null())
            .add(Expr::col(Sessions::ArchiveInc).lt(to.inc))
            .add(
                Expr::col(Sessions::ArchiveInc)
                    .eq(to.inc)
                    .and(Expr::col(Sessions::ArchiveAt).lte(to.at.as_millis())),
            );
        let moved = tx.run(
            Query::update()
                .table(Sessions::Table)
                .value(Sessions::ArchiveKey, to.key.as_str())
                .value(Sessions::ArchiveInc, to.inc)
                .value(Sessions::ArchiveAt, to.at.as_millis())
                .and_where(Expr::col(Sessions::Id).eq(session))
                .and_where(Expr::col(Sessions::Box).eq(box_id.get()))
                .cond_where(behind),
        )? > 0;
        if moved {
            reschedule(&tx, box_id, to.at, None)?;
        }
        tx.commit()?;
        drop(conn);
        Ok(moved)
    }

    /// Makes Box `id`'s archive due at `next`, or owed no more when none, if it fell due no later
    /// than `seen`; a due marked after `seen` stays.
    pub(crate) fn reschedule_archive(
        &self,
        id: BoxId,
        seen: Ts,
        next: Option<Ts>,
    ) -> Result<(), StoreError> {
        self.check_write(id);
        let conn = self.writer();
        reschedule(&*conn, id, seen, next)?;
        drop(conn);
        Ok(())
    }

    /// The archive Session `session` points at.
    #[cfg(test)]
    pub(crate) fn archive_pointer(
        &self,
        session: i64,
    ) -> Result<Option<ArchivePointer>, StoreError> {
        let conn = self.reader()?;
        let row: Option<(Option<String>, Option<i64>, Option<i64>)> = conn
            .row(
                Query::select()
                    .columns([
                        Sessions::ArchiveKey,
                        Sessions::ArchiveInc,
                        Sessions::ArchiveAt,
                    ])
                    .from(Sessions::Table)
                    .and_where(Expr::col(Sessions::Id).eq(session)),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            Some((Some(key), Some(inc), Some(at))) => Some(ArchivePointer {
                key,
                inc,
                at: Ts::from_millis(at),
            }),
            _ => None,
        })
    }
}

/// [`Store::reschedule_archive`] through `conn`.
fn reschedule(conn: &dyn Db, id: BoxId, seen: Ts, next: Option<Ts>) -> Result<(), StoreError> {
    conn.run(
        Query::update()
            .table(BoxState::Table)
            .value(BoxState::ArchiveDue, Value::BigInt(next.map(Ts::as_millis)))
            .and_where(Expr::col(BoxState::Box).eq(id.get()))
            .and_where(Expr::col(BoxState::ArchiveDue).lte(seen.as_millis())),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
