//! A close the controller stopped part-way, mid-release, is finished by the next controller.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::hands::HandsState;

use super::boxwork::fx_woken;
use super::{req, row, run};
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;

#[test]
fn a_close_cut_mid_release_is_finished_after_a_restart() {
    let f = fx_woken("ahead=0 dirty=0");
    f.driver
        .on_release(|| std::panic::resume_unwind(Box::new("process died mid-release")));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&f.cx, &req(false)));
    }));
    assert!(crashed.is_err());
    f.driver.on_release(|| {});
    assert_ne!(row(&f).lifecycle, BoxLifecycle::Closed);
    let store = f.cx.store().unwrap();
    assert_eq!(fail_interrupted(&f.cx, store).unwrap(), [f.id]);
    resume_setup(&f.cx, store, f.id).unwrap();
    let closed = row(&f);
    assert_eq!(closed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(closed.hands.map(|b| b.state), Some(HandsState::Deleted));
    assert!(fail_interrupted(&f.cx, store).unwrap().is_empty());
}
