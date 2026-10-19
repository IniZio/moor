//! `start --parent` records the parent Session, and refuses a bad one before any Box exists.

use moor_core::{ErrorKind, SessionId, StartReq};

use super::{req, world};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

/// A start request for `name` under `parent`.
fn child(name: &str, parent: i64) -> StartReq {
    StartReq {
        parent: Some(SessionId::new(parent)),
        ..req(name)
    }
}

/// The parent of every Session of the Box `id`.
fn parents(w: &super::World, id: i64) -> Vec<Option<i64>> {
    let store = w.cx.store().unwrap();
    let reader = store.reader().unwrap();
    let mut stmt = reader
        .prepare("SELECT parent_session FROM sessions WHERE box = ? ORDER BY id")
        .unwrap();
    stmt.query_map([id], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn start_with_a_parent_records_it_on_the_first_session() {
    let w = world(FakeDriver::new());
    let parent =
        w.cx.store()
            .unwrap()
            .open_host_session("/w", None, None)
            .unwrap();
    let v = run(&w.cx, &child("demo", parent)).unwrap();
    assert_eq!(parents(&w, v.task.id), vec![Some(parent)]);
}

#[test]
fn start_without_a_parent_leaves_it_null() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(parents(&w, v.task.id).iter().all(Option::is_none));
}

#[test]
fn start_with_an_unknown_parent_is_refused_before_any_box() {
    let w = world(FakeDriver::new());
    let err = run(&w.cx, &child("demo", 999)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    let store = w.cx.store().unwrap();
    assert!(store
        .open_task_by_name(&req("demo").name)
        .unwrap()
        .is_none());
    assert!(w.driver.calls().is_empty());
}

#[test]
fn start_with_an_ended_parent_is_refused_before_any_box() {
    let w = world(FakeDriver::new());
    let store = w.cx.store().unwrap();
    let parent = store.open_host_session("/w", None, None).unwrap();
    store.end_host_session(parent).unwrap();
    let err = run(&w.cx, &child("demo", parent)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(store
        .open_task_by_name(&req("demo").name)
        .unwrap()
        .is_none());
    assert!(w.driver.calls().is_empty());
}
