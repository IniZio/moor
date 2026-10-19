//! Reads of the Box lifecycle and primary Session state a start leaves on a row.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::session_state::SessionState;

use crate::store::BoxRow;

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
    let first = crate::ops::start::run(&w.cx, &super::req("demo")).unwrap();
    let id = moor_core::BoxId::new(first.boxed.id);
    let store = w.cx.store().unwrap();
    let conn = store.writer().unwrap();
    conn.execute("UPDATE events SET session = NULL WHERE box = ?", [id.get()])
        .unwrap();
    conn.execute("DELETE FROM sessions WHERE box = ?", [id.get()])
        .unwrap();
    drop(conn);
    let row = store.task(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.session, SessionState::Starting);
    assert!(crate::ops::start::launch::working(&row));
}
