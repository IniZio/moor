//! A retry that forks a kept layer moves the Box onto new hands, so nothing late about the machine
//! it left lands on the fork.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::seam::driver::ExecOutcome;
use moor_core::BoxHandle;

use super::{req, row, run, saved_layer, started, world, TWO_LINES};
use crate::store::StoreError;
use crate::testing::driver::FakeDriver;

#[test]
fn a_retry_fork_gets_new_hands_and_a_late_write_for_the_old_ones_is_refused() {
    let driver = FakeDriver::new()
        .with_checkpoints()
        .exec_results([Ok(ExecOutcome::Exited { code: 1 })]);
    let w = world(TWO_LINES, driver);
    let id = started(&w);
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::SetupFailed);
    saved_layer(&w, id, 1);

    run(&w.cx, &req()).unwrap();

    let after = row(&w, id);
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
    assert!(w.driver.calls().iter().any(|c| c.starts_with("fork")));
    let hands = after.hands.unwrap();
    assert_eq!(hands.inc, HandsInc::new(2));
    assert_ne!(hands.handle, Some(BoxHandle::new("box-1")), "on the fork");
    let store = w.cx.store().unwrap();
    assert_eq!(
        store.hands_history(id),
        [(1, "gone".to_owned(), true), (2, "ready".to_owned(), false)]
    );
    let late = store.apply(Decision::new(id).hands_input(HandsInc::FIRST, HandsInput::Sleep));
    assert!(matches!(late, Err(StoreError::Stale { .. })), "{late:?}");
    assert_eq!(row(&w, id).hands.unwrap().handle, hands.handle);
}
