//! Reads of one kind of a Box's events.

use moor_core::domain::event::close_reason::CloseReason;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::Events;
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::select::{events_of, limit};
use super::{event_from_row, EventRow, Store, StoreError};
use crate::store::db::DbExt as _;

impl Store {
    /// The newest `n` events of `kind` for a box, ascending by id; empty when it has had none.
    pub(crate) fn newest_events_of(
        &self,
        id: BoxId,
        kind: &str,
        n: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            events_of(id)
                .and_where(Expr::col(Events::Kind).eq(kind))
                .order_by(Events::Id, Order::Desc)
                .limit(limit(n)),
            event_from_row,
        )?;
        let mut newest = rows.collect::<Result<Vec<_>, _>>()?;
        newest.reverse();
        Ok(newest)
    }

    /// When the newest event of Box `id` whose kind is one of `kinds` was recorded; none when it
    /// has had none.
    pub(crate) fn last_event_at(
        &self,
        id: BoxId,
        kinds: &[&str],
    ) -> Result<Option<Ts>, StoreError> {
        let q = Query::select()
            .expr(Expr::col(Events::Ts).max())
            .from(Events::Table)
            .and_where(Expr::col(Events::Box).eq(id.get()))
            .and_where(Expr::col(Events::Kind).is_in(kinds.iter().copied()))
            .take();
        let at: Option<i64> = self.reader()?.row(q, |r| r.get(0))?;
        Ok(at.map(Ts::from_millis))
    }

    /// Why Box `id` closed, as its `closed` event says; none when it has not closed, or closed
    /// before every close named a reason.
    pub(crate) fn closed_reason(&self, id: BoxId) -> Result<Option<CloseReason>, StoreError> {
        let Some(e) = self.newest_event_of(id, "closed")? else {
            return Ok(None);
        };
        let said = |key: &str| e.payload.get(key).and_then(serde_json::Value::as_str);
        Ok(said("reason").and_then(|r| CloseReason::from_event(r, said("by"))))
    }

    /// How many events of `kind` a box has had.
    pub(crate) fn count_events_of(&self, id: BoxId, kind: &str) -> Result<u64, StoreError> {
        let q = Query::select()
            .expr(Expr::col(Events::Id).count())
            .from(Events::Table)
            .and_where(Expr::col(Events::Box).eq(id.get()))
            .and_where(Expr::col(Events::Kind).eq(kind))
            .take();
        let n: i64 = self.reader()?.row(q, |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }
}
