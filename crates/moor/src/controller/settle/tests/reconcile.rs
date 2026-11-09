//! The periodic reconcile pass notices lost VMs without a read.

use super::*;

/// The hands state of Box `id`, read through a fresh context over the same state.
fn hands_state(
    dir: &Path,
    driver: &Arc<FakeDriver>,
    id: BoxId,
) -> moor_core::domain::hands::HandsState {
    let store = view(dir, driver);
    let row = store.store().unwrap().box_row(id).unwrap().unwrap();
    row.hands.unwrap().state
}

fn lease_of(cx: &Cx, id: BoxId) -> moor_core::BoxHandle {
    cx.store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .hands
        .unwrap()
        .handle
        .unwrap()
}

#[test]
fn reconcile_pass_marks_a_ready_box_gone_when_its_lease_disappears_without_any_read() {
    use moor_core::domain::hands::HandsState;
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", "s-a");
    let moor = Moor::from_cx(cx);
    assert!(moor.reconcile_open(&|| false).is_empty());
    assert_eq!(hands_state(dir.path(), &driver, id), HandsState::Ready);
    driver.forget(&lease_of(&view(dir.path(), &driver), id));
    assert!(moor.reconcile_open(&|| false).is_empty());
    assert_eq!(hands_state(dir.path(), &driver, id), HandsState::Gone);
}

#[test]
fn reconcile_pass_marks_nothing_when_the_listing_fails() {
    use moor_core::domain::hands::HandsState;
    use moor_core::seam::driver::DriverError;
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", "s-a");
    driver.forget(&lease_of(&cx, id));
    driver.fail_list_always(DriverError::Failed {
        summary: "node away".into(),
    });
    let said = Moor::from_cx(cx).reconcile_open(&|| false);
    assert!(
        said.iter()
            .any(|m| m.contains("box states left as they were")),
        "{said:?}"
    );
    assert_eq!(hands_state(dir.path(), &driver, id), HandsState::Ready);
}

#[test]
fn reconcile_pass_skips_a_busy_box_and_catches_it_on_the_next_pass() {
    use moor_core::domain::hands::HandsState;
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", "s-a");
    driver.forget(&lease_of(&cx, id));
    let owners = Arc::clone(cx.owners_arc());
    let moor = Moor::from_cx(cx);
    let held = owners.hold(id, Cmd::Sleep);
    moor.reconcile_open(&|| false);
    assert_eq!(hands_state(dir.path(), &driver, id), HandsState::Ready);
    drop(held);
    moor.reconcile_open(&|| false);
    assert_eq!(hands_state(dir.path(), &driver, id), HandsState::Gone);
}

#[test]
fn reconcile_runs_on_the_first_tick_then_once_per_period() {
    let every =
        crate::controller::settle::Every::new(Duration::from_secs(60), Duration::from_secs(5));
    let due: Vec<bool> = (0..25).map(|_| every.due()).collect();
    let at: Vec<usize> = (0..25).filter(|&i| due.get(i) == Some(&true)).collect();
    assert_eq!(at, [0, 12, 24]);
}
