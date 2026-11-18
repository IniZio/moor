//! A Box whose places died while its agent worked is not resumed on its own: it sleeps, stuck,
//! until a person wakes it.

use super::*;
use crate::cx::owners::Cmd;
use crate::ops::common::promote::promote_waiting;
use crate::ops::common::reconcile;
use crate::ops::common::wake::wake_box_if_asleep;
use moor_core::name::NodeName;

const DIED: &str = "the work place exited with code 137, not for memory";

/// A working Box placed on the test node, with its machine up.
fn placed(cx: &Cx, driver: &FakeDriver) -> BoxId {
    let id = working(cx, driver, "a", None);
    let node = NodeName::parse("local-container").unwrap();
    cx.store().unwrap().set_box_node(id, Some(&node)).unwrap();
    id
}

fn row(cx: &Cx, id: BoxId) -> BoxRow {
    cx.store().unwrap().box_row(id).unwrap().unwrap()
}

/// The Box's places die behind moor's back, and a reconcile sees it.
fn dies(cx: &Cx, driver: &FakeDriver, id: BoxId) {
    let handle = row(cx, id).hands.unwrap().handle.unwrap();
    driver.die_behind(&handle, DIED);
    reconcile::run(cx);
}

/// The Box's machine stops with no word of why, as when its host restarts.
fn stops(cx: &Cx, driver: &FakeDriver, id: BoxId) {
    let handle = row(cx, id).hands.unwrap().handle.unwrap();
    driver.stop_behind(&handle);
    reconcile::run(cx);
}

/// A promotion pass over the Box's node: the automatic resume.
fn promoted(cx: &Cx, id: BoxId) {
    let store = cx.store().unwrap();
    let node = store.box_spec(id).unwrap().node.unwrap();
    promote_waiting(cx, store, &node);
}

/// A person wakes the Box.
fn person_wakes(cx: &Cx, id: BoxId) {
    let _turn = cx.turn(id, Cmd::Wake).unwrap();
    wake_box_if_asleep(cx, cx.store().unwrap(), &row(cx, id)).unwrap();
}

/// The Box sleeps, not queued, saying how its places died, and the owner was told.
fn assert_stuck(cx: &Cx, id: BoxId) {
    let r = row(cx, id);
    assert_eq!(r.lifecycle, BoxLifecycle::Asleep, "{:?}", r.state_reason);
    let why = r.state_reason.unwrap_or_default();
    assert!(why.contains("moor did not resume it"), "{why}");
    assert!(why.contains(DIED), "{why}");
    let store = cx.store().unwrap();
    assert!(store.resume_first(id).unwrap().is_none());
    let told = store.newest_event_of(id, "death_capped").unwrap().unwrap();
    assert_eq!(told.payload.get("reason"), Some(&DIED.into()));
}

/// The Box died and waits in the queue to be resumed.
fn assert_queued(cx: &Cx, id: BoxId) {
    let r = row(cx, id);
    assert_eq!(r.lifecycle, BoxLifecycle::Pending, "{:?}", r.state_reason);
    assert!(cx.store().unwrap().resume_first(id).unwrap().is_some());
}

/// The Box was resumed and runs.
fn assert_running(cx: &Cx, id: BoxId) {
    assert_eq!(row(cx, id).lifecycle, BoxLifecycle::Ready);
}

#[test]
fn the_first_death_is_not_resumed_and_the_box_is_stuck() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = placed(&cx, &driver);
    dies(&cx, &driver, id);
    assert_stuck(&cx, id);
    promoted(&cx, id);
    assert_eq!(row(&cx, id).lifecycle, BoxLifecycle::Asleep);
}

#[test]
fn a_machine_that_stopped_with_no_reason_is_not_a_death() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = placed(&cx, &driver);
    for _ in 0..4 {
        stops(&cx, &driver, id);
        assert_queued(&cx, id);
        promoted(&cx, id);
        assert_running(&cx, id);
    }
}

#[test]
fn a_person_waking_a_stuck_box_brings_it_back_and_the_next_death_sticks_again() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = placed(&cx, &driver);
    dies(&cx, &driver, id);
    assert_stuck(&cx, id);
    person_wakes(&cx, id);
    assert_running(&cx, id);
    dies(&cx, &driver, id);
    assert_stuck(&cx, id);
}
