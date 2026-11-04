//! Reads of the Box lifecycle and primary Session state a start leaves on a row.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::session_state::SessionState;

use crate::store::db::DbExt as _;
use crate::store::BoxRow;
use moor_schema::tables::{Events, Sessions};
use sea_query::{Expr, ExprTrait as _, Query};

pub(super) const WORKING: (BoxLifecycle, SessionState) =
    (BoxLifecycle::Ready, SessionState::Working);

pub(super) fn working(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready && !row.session.is_quiet()
}

pub(super) fn idle(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready && row.session.is_quiet()
}

#[test]
fn ready_box_without_a_session_row_counts_as_working() {
    let w = super::world(crate::testing::driver::FakeDriver::new());
    let first = crate::ops::start::run(&w.cx, &super::agentless_req("demo")).unwrap();
    let id = moor_core::BoxId::new(first.boxed.id);
    let store = w.cx.store().unwrap();
    let conn = store.writer();
    conn.run(
        Query::update()
            .table(Events::Table)
            .value(Events::Session, Option::<i64>::None)
            .and_where(Expr::col(Events::Box).eq(id.get())),
    )
    .unwrap();
    conn.run(
        Query::delete()
            .from_table(Sessions::Table)
            .and_where(Expr::col(Sessions::Box).eq(id.get())),
    )
    .unwrap();
    drop(conn);
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.session, SessionState::Starting);
    assert!(crate::ops::start::launch::working(&row));
}
