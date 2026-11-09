//! Read-only lookups by box name and live-box scans, built on the shared box row mapping.

use moor_core::controller::ServerFrame;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::hands::HandsState;
use moor_core::request::Pins;
use moor_core::{BoxId, BoxName, SessionId};

use moor_schema::tables::{BoxHands, BoxState, Boxes, Events, Sessions};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::{
    box_from_row, box_select, event_from_row, events_of, newest_event, BoxRow, EventRow, HandsRow,
    Store, StoreError, EVENT_COLUMNS,
};
use crate::store::db::{self, Db, DbExt as _};

/// The Box reads' columns where `filter` holds, in `order` by id, every row mapped.
fn query_boxes(
    conn: &dyn Db,
    filter: Expr,
    order: Order,
    limit: Option<u64>,
) -> Result<Vec<BoxRow>, StoreError> {
    let mut q = box_select();
    q.and_where(filter)
        .order_by((Boxes::Table, Boxes::Id), order);
    if let Some(n) = limit {
        q.limit(n);
    }
    conn.rows(q, box_from_row)?.collect()
}

/// A column of `boxes`, as the Box reads name it.
const fn b(c: Boxes) -> (Boxes, Boxes) {
    (Boxes::Table, c)
}

/// Lifecycles of a Box that is not open.
pub(in crate::store) const SHUT: [&str; 2] = ["closed", "rejected"];

/// Box states that count toward capacity, as SQL parameters.
const LIVE_STATES: [&str; 3] = [
    HandsState::Starting.as_str(),
    HandsState::Ready.as_str(),
    HandsState::Asleep.as_str(),
];

/// An event with the name of the Box that owns it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NamedEvent {
    /// The stored event.
    pub(crate) row: EventRow,
    /// Name of the owning Box.
    pub(crate) box_name: String,
    /// The Box's Session the event belongs to, when one had started.
    pub(crate) session: Option<SessionId>,
}

impl NamedEvent {
    /// The wire frame for this event.
    pub(crate) fn into_frame(self) -> ServerFrame {
        ServerFrame::Event {
            cursor: self.row.id,
            r#box: Some(self.box_name),
            box_id: Some(self.row.box_id),
            session: self.session,
            kind: self.row.kind,
            payload: self.row.payload,
            ts: self.row.ts.as_millis(),
        }
    }
}

impl Store {
    /// Events with id greater than `after`, ascending by id, at most `limit`.
    pub(crate) fn events_after(
        &self,
        after: i64,
        limit: usize,
    ) -> Result<Vec<NamedEvent>, StoreError> {
        let e = |c| (Events::Table, c);
        let started = Query::select()
            .column(Sessions::Id)
            .from(Sessions::Table)
            .and_where(Expr::col(Sessions::Box).equals(e(Events::Box)))
            .and_where(Expr::col(Sessions::StartedAt).lte(Expr::col(e(Events::Ts))))
            .order_by(Sessions::Ordinal, Order::Desc)
            .limit(1)
            .take();
        let q = Query::select()
            .columns(EVENT_COLUMNS.map(e))
            .column(b(Boxes::Name))
            .expr(Expr::col(e(Events::Session)).if_null(db::sub(started)))
            .from(Events::Table)
            .inner_join(Boxes::Table, Expr::col(b(Boxes::Id)).equals(e(Events::Box)))
            .and_where(Expr::col(e(Events::Id)).gt(after))
            .order_by(e(Events::Id), Order::Asc)
            .limit(super::limit(limit))
            .take();
        let rows = self.reader()?.rows(q, |r| {
            Ok(NamedEvent {
                row: event_from_row(r)?,
                box_name: r.get(5)?,
                session: r.get::<_, Option<i64>>(6)?.map(SessionId::new),
            })
        })?;
        rows.collect()
    }

    /// The box's latest event of `kind`; none when it has had none.
    pub(crate) fn last_event_of(
        &self,
        id: BoxId,
        kind: &str,
    ) -> Result<Option<EventRow>, StoreError> {
        newest_event(
            &self.reader()?,
            events_of(id).and_where(Expr::col(Events::Kind).eq(kind)),
        )
    }

    /// Highest event id, or 0 when no event is stored.
    pub(crate) fn last_event_id(&self) -> Result<i64, StoreError> {
        let max: Option<i64> = self.reader()?.row(
            Query::select()
                .expr(Expr::col(Events::Id).max())
                .from(Events::Table),
            |r| r.get(0),
        )?;
        Ok(max.unwrap_or(0))
    }

    /// Boxes that are neither closed nor rejected, by id ascending.
    pub(crate) fn open_boxes(&self) -> Result<Vec<BoxRow>, StoreError> {
        let open = Expr::col((BoxState::Table, BoxState::State)).is_not_in(SHUT);
        query_boxes(&self.reader()?, open, Order::Asc, None)
    }

    /// This store with each name in `pins` meaning only its pinned Box.
    #[must_use]
    pub(crate) fn pinned(mut self, pins: Pins) -> Self {
        self.pins = pins;
        self
    }

    /// The live (neither closed nor rejected) box with this name.
    pub(crate) fn open_box_by_name(&self, name: &BoxName) -> Result<Option<BoxRow>, StoreError> {
        if let Some(&id) = self.pins.get(name) {
            let open =
                |r: &BoxRow| !matches!(r.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected);
            return Ok(self.box_row(id)?.filter(open));
        }
        let named = Expr::col((BoxState::Table, BoxState::OpenName)).eq(name.as_str());
        let rows = query_boxes(&self.reader()?, named, Order::Desc, Some(1))?;
        Ok(rows.into_iter().next())
    }

    /// The newest box with this name, open or closed.
    pub(crate) fn latest_box_by_name(&self, name: &BoxName) -> Result<Option<BoxRow>, StoreError> {
        if let Some(&id) = self.pins.get(name) {
            return self.box_row(id);
        }
        let named = Expr::col(b(Boxes::Name)).eq(name.as_str());
        let rows = query_boxes(&self.reader()?, named, Order::Desc, Some(1))?;
        Ok(rows.into_iter().next())
    }

    /// Boxes whose state counts toward capacity (starting, ready, asleep), with their box.
    pub(crate) fn box_rows_live(&self) -> Result<Vec<(BoxId, HandsRow)>, StoreError> {
        let live = Expr::col((BoxHands::Table, BoxHands::State)).is_in(LIVE_STATES);
        let rows = query_boxes(&self.reader()?, live, Order::Asc, None)?;
        Ok(rows
            .into_iter()
            .filter_map(|t| t.hands.map(|b| (t.id, b)))
            .collect())
    }

    /// Boxes queued for a box, oldest id first.
    #[cfg(test)]
    pub(crate) fn waiting_boxes(&self) -> Result<Vec<BoxRow>, StoreError> {
        let pending =
            Expr::col((BoxState::Table, BoxState::State)).eq(BoxLifecycle::Pending.as_str());
        query_boxes(&self.reader()?, pending, Order::Asc, None)
    }
}

mod refs;

#[cfg(test)]
mod tests;
