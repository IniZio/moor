//! Reconcile corrects a Box its host lists as stopped.

use super::*;

#[test]
fn reconcile_sleeps_a_working_box_its_host_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    let handle = cx
        .store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .boxed
        .unwrap()
        .handle
        .unwrap();
    driver.stop_behind(&handle);
    assert_eq!(box_state(&cx), BoxState::Asleep);
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Asleep);
    assert_eq!(row.boxed.unwrap().handle, Some(handle));
}

#[test]
fn reconcile_leaves_a_box_alone_while_an_op_holds_its_lock() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    driver.stop_behind(&row.boxed.unwrap().handle.unwrap());
    let held = crate::ops::common::boxlock::BoxLock::acquire(
        cx.clock(),
        cx.state_dir().unwrap(),
        id,
        "sleep",
        std::time::Duration::ZERO,
    )
    .unwrap();
    assert_eq!(
        box_state(&cx),
        BoxState::Ready,
        "a machine that shows paused mid-suspend is that op's to record"
    );
    assert!(cx
        .store()
        .unwrap()
        .newest_event_of(id, "asleep")
        .unwrap()
        .is_none());
    drop(held);
    assert_eq!(box_state(&cx), BoxState::Asleep);
}

#[test]
fn reconcile_leaves_a_running_box_its_host_lists() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    assert_eq!(box_state(&cx), BoxState::Ready);
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
}

#[test]
fn reconcile_keeps_a_box_its_host_no_longer_lists() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    let handle = cx
        .store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .boxed
        .unwrap()
        .handle
        .unwrap();
    driver.forget(&handle);
    assert_eq!(box_state(&cx), BoxState::Ready);
}
