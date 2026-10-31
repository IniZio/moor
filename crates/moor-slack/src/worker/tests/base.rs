//! The `base` of a Box row: the start request carries it, and a base the forge refuses is told
//! to the thread like any refused start.
#![allow(clippy::panic, reason = "test code")]

use moor_core::request::Request;

use super::{attempt_with, cmds, db, named, recorded_start, refused, started, state};
use crate::command::BoxEvent;
use crate::worker::Attempt;

#[test]
fn the_start_carries_the_base_the_box_row_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let mut row = recorded_start(&db);
    let plain = attempt_with(&db, &row, Attempt::First, &mut |req| match req {
        Request::Start(s) => named("start", s.name.as_str()),
        _ => refused("show", "unexpected"),
    });
    let Some(Request::Start(req)) = plain.calls.first() else {
        panic!("no start: {:?}", cmds(&plain));
    };
    assert_eq!(req.base, None);

    row = started(&db, "Ev2", "11.5");
    db.forget_starting(&row.box_name).unwrap();
    row.base = Some("moor-setup".into());
    db.insert_starting(&row, 1_000).unwrap();
    let row = db.thread_boxes("C1", "11.5").unwrap().remove(0);
    let seen = attempt_with(&db, &row, Attempt::First, &mut |req| match req {
        Request::Start(s) => named("start", s.name.as_str()),
        _ => refused("show", "unexpected"),
    });
    let Some(Request::Start(req)) = seen.calls.first() else {
        panic!("no start: {:?}", cmds(&seen));
    };
    assert_eq!(req.base.as_deref(), Some("moor-setup"));
}

#[test]
fn a_base_the_forge_cannot_resolve_reaches_the_thread_as_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let mut row = recorded_start(&db);
    db.forget_starting(&row.box_name).unwrap();
    row.base = Some("nope".into());
    db.insert_starting(&row, 1_000).unwrap();
    let seen = attempt_with(&db, &row, Attempt::First, &mut |_| {
        refused("start", "no ref nope in acme/api")
    });
    assert_eq!(seen.calls.len(), 1);
    assert_eq!(seen.replies.len(), 1);
    assert!(seen.replies[0].2.contains("no ref nope in acme/api"));
    assert_eq!(state(&db, "C1", "10.5"), None);
    assert!(db.has_notice(&row, BoxEvent::NotStarted, None).unwrap());
}

#[test]
fn the_start_carries_the_node_the_box_row_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let mut row = recorded_start(&db);
    db.forget_starting(&row.box_name).unwrap();
    row.node = Some("hv".into());
    db.insert_starting(&row, 1_000).unwrap();
    let row = db.thread_boxes("C1", "10.5").unwrap().remove(0);
    let seen = attempt_with(&db, &row, Attempt::First, &mut |req| match req {
        Request::Start(s) => named("start", s.name.as_str()),
        _ => refused("show", "unexpected"),
    });
    let Some(Request::Start(req)) = seen.calls.first() else {
        panic!("no start: {:?}", cmds(&seen));
    };
    assert_eq!(req.node.as_deref(), Some("hv"));
}
