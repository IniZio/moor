//! A start asks the provider about the machine of the Box it attaches to.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::seam::driver::{Driver as _, DriverError};
use moor_core::{BoxHandle, BoxId};

use super::relaunch::{gone, started};
use super::{req, World};
use crate::ops::start::run;

fn handle_of(w: &World, id: BoxId) -> Option<BoxHandle> {
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    row.boxed.unwrap().handle
}

fn kinds(w: &World, id: BoxId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn vanish(w: &World) {
    w.driver.release(&BoxHandle::new("box-1")).unwrap();
    gone(w);
}

#[test]
fn a_start_alone_gets_a_new_machine_for_a_box_whose_machine_vanished() {
    let (w, engine, id) = started();
    vanish(&w);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(handle_of(&w, id), Some(BoxHandle::new("box-2")));
    let kinds = kinds(&w, id);
    assert!(kinds.iter().any(|k| k == "box_gone"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "disk_lost"), "{kinds:?}");
    assert!(v.agent.launched);
    assert_eq!(engine.launches().last().unwrap().handle.as_str(), "box-2");
}

#[test]
fn a_start_that_cannot_list_keeps_the_machine_and_says_so() {
    let (w, _engine, id) = started();
    w.driver.fail_list_always(DriverError::Failed {
        summary: "provider down".into(),
    });
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(handle_of(&w, id), Some(BoxHandle::new("box-1")));
    assert!(!kinds(&w, id).iter().any(|k| k == "box_gone"));
    let warnings = w.cx.take_warnings();
    assert!(
        warnings.iter().any(|x| x.code == "reconcile_skipped"),
        "{warnings:?}"
    );
}

#[test]
fn a_start_wakes_a_box_whose_machine_stopped_behind_moor() {
    let (w, _engine, id) = started();
    w.driver.stop_behind(&BoxHandle::new("box-1"));
    run(&w.cx, &req("demo")).unwrap();
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.boxed.unwrap().state, BoxState::Ready);
    let kinds = kinds(&w, id);
    assert!(kinds.iter().any(|k| k == "asleep"), "{kinds:?}");
    assert!(w.driver.calls().iter().any(|c| c.starts_with("wake")));
}
