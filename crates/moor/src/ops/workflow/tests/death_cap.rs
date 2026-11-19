//! A run whose Box's places died fails at its step at once, not at its timeout.
#![allow(clippy::panic, reason = "a test names what it found when it fails")]

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::workflow::{Plan, RunEnd, RunOutcome, RunState, WorkflowId};
use moor_core::BoxId;

use super::{rig_driving, Rig, FIRST_DUE};
use crate::cx::owners::Cmd;
use crate::ops::common::promote::promote_waiting;
use crate::ops::common::reconcile;
use crate::ops::common::wake::wake_box_if_asleep;
use crate::testing::driver::FakeDriver;

const DIED: &str = "the Box's container stopped (exit 137)";

/// A rig whose driver lists its Boxes by handle, so a reconcile can see their places die.
fn rig_that_notices_deaths() -> Rig {
    rig_driving(UserConfig::default(), FakeDriver::listed_by_handle)
}

/// A workflow of one step whose run has started its Box and works in it.
fn working_run(r: &Rig) -> (WorkflowId, BoxId) {
    r.answer_repo();
    let id = r.put(300);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let box_id = r.latest_box(id);
    assert!(matches!(r.latest(id).state, RunState::Running { .. }));
    (id, box_id)
}

fn row(r: &Rig, id: BoxId) -> crate::store::BoxRow {
    r.cx.store().unwrap().box_row(id).unwrap().unwrap()
}

/// The Box's places die behind moor's back, and a reconcile sees it.
fn dies(r: &Rig, id: BoxId) {
    let handle = row(r, id).hands.unwrap().handle.unwrap();
    r.driver.die_behind(&handle, DIED);
    reconcile::run(&r.cx);
}

/// A promotion pass over the Box's node: the automatic resume.
fn resumed(r: &Rig, id: BoxId) {
    let store = r.cx.store().unwrap();
    let node = store.box_spec(id).unwrap().node.unwrap();
    promote_waiting(&r.cx, store, &node);
}

/// The Box dies, and moor does not resume it.
fn dies_and_stays_down(r: &Rig, id: BoxId) {
    dies(r, id);
    assert_eq!(row(r, id).lifecycle, BoxLifecycle::Asleep);
    assert!(r
        .cx
        .store()
        .unwrap()
        .newest_event_of(id, "death_capped")
        .unwrap()
        .is_some());
}

fn failed_reason(r: &Rig, id: WorkflowId) -> (moor_core::workflow::StepId, String) {
    match r.latest(id).state {
        RunState::Done(RunEnd::Ended {
            outcome: RunOutcome::Failed { at, reason },
            ..
        }) => (at, reason),
        other => panic!("not failed: {other:?}"),
    }
}

#[test]
fn a_run_fails_at_its_step_the_tick_after_its_box_dies() {
    let r = rig_that_notices_deaths();
    let (id, box_id) = working_run(&r);
    dies_and_stays_down(&r, box_id);

    r.pass();

    let (at, reason) = failed_reason(&r, id);
    assert_eq!(at, Plan::single_id());
    assert!(reason.contains("moor did not resume it"), "{reason}");
    assert!(reason.contains(DIED), "{reason}");
    let store = r.cx.store().unwrap();
    let told = store
        .newest_event_of(box_id, "run_failed")
        .unwrap()
        .unwrap();
    assert_eq!(told.payload.get("outcome"), Some(&"failed".into()));
    assert_eq!(told.payload.get("reason"), Some(&reason.into()));
    assert_eq!(told.payload.get("step"), Some(&at.as_str().into()));
    assert_eq!(row(&r, box_id).lifecycle, BoxLifecycle::Asleep, "not woken");
    assert!(store
        .newest_event_of(box_id, "woke")
        .unwrap()
        .is_none_or(|w| {
            w.id < store
                .newest_event_of(box_id, "death_capped")
                .unwrap()
                .unwrap()
                .id
        }));
}

#[test]
fn a_machine_that_stopped_with_no_reason_leaves_the_run_going() {
    let r = rig_that_notices_deaths();
    let (id, box_id) = working_run(&r);
    let handle = row(&r, box_id).hands.unwrap().handle.unwrap();
    r.driver.stop_behind(&handle);
    reconcile::run(&r.cx);
    r.pass();
    assert!(matches!(r.latest(id).state, RunState::Running { .. }));
    resumed(&r, box_id);
    r.pass();
    assert!(matches!(r.latest(id).state, RunState::Running { .. }));
}

#[test]
fn a_person_waking_the_stuck_box_before_the_tick_keeps_the_run_going() {
    let r = rig_that_notices_deaths();
    let (id, box_id) = working_run(&r);
    dies_and_stays_down(&r, box_id);
    let _turn = r.cx.turn(box_id, Cmd::Wake).unwrap();
    wake_box_if_asleep(&r.cx, r.cx.store().unwrap(), &row(&r, box_id)).unwrap();

    r.pass();

    assert!(matches!(r.latest(id).state, RunState::Running { .. }));
}
