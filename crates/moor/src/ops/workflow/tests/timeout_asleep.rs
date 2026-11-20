//! A run that times out while its Box sleeps ends without waking the Box.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInput, HandsState};
use moor_core::domain::session_state::SessionState;

use super::{rig, FIRST_DUE};
use crate::testing::runs::timed_out;

#[test]
fn a_run_past_its_timeout_leaves_its_asleep_box_asleep_and_ends_its_session() {
    let r = rig();
    r.answer_repo();
    let id = r.put(30);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let box_id = r.latest_box(id);
    let store = r.cx.store().unwrap();
    let row = store.box_row(box_id).unwrap().unwrap();
    let sleep = Decision::new(box_id)
        .lifecycle(BoxLifecycleInput::Sleep {
            auto: false,
            reason: None,
        })
        .hands_input(row.hands_inc(), HandsInput::Sleep);
    store.apply(sleep).unwrap();

    r.advance_to(FIRST_DUE + 1_000 + 31 * 60_000);
    r.pass();
    assert_eq!(
        r.latest(id).state,
        timed_out(box_id.get(), FIRST_DUE + 1_000 + 31 * 60_000)
    );
    let row = store.box_row(box_id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Asleep);
    assert_eq!(row.hands.unwrap().state, HandsState::Asleep);
    let session = store.box_sessions(box_id).unwrap().pop().unwrap();
    assert_eq!(session.state, SessionState::Ended);
    assert!(store.live_launch(box_id).unwrap().is_none());
}
