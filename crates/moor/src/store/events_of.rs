//! Reads of one kind of a Box's events.

use moor_core::BoxId;
use moor_schema::tables::Events;
use sea_query::{Expr, ExprTrait as _, Order};

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
}
