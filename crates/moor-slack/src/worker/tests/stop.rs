//! The Stop button: what a click queues, and the interrupt the worker sends for it.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use std::cell::RefCell;

use moor_core::name::BoxId;
use moor_core::request::{Request, SteerReq};
use moor_core::response::Response;
use serde_json::json;

use super::{db, hung_up, recorded_start, refused, with_fx, Answer, Seen};
use crate::db::{Db, SlackBox};
use crate::socket::{Click, Press};
use crate::worker::answer::click;
use crate::worker::stop::stop;
use crate::worker::Job;

/// A bound Box in thread `10.5` whose status message is `s7`.
fn shown(db: &Db) -> SlackBox {
    let row = recorded_start(db);
    db.bind(&row.box_name, BoxId::new(7), None).unwrap();
    db.set_status(row.box_name.as_str(), "s7", "*b*: working")
        .unwrap();
    row
}

/// A press of Stop on message `s7` by `user`, on the button of Box `name`.
fn press(name: &str, user: &str, envelope: &str) -> Click {
    Click {
        envelope_id: envelope.into(),
        channel: "C1".into(),
        user: user.into(),
        message_ts: "s7".into(),
        thread_ts: Some("10.5".into()),
        press: Press::Stop(name.into()),
    }
}

/// Runs the queued stop `job` as the worker, answering each request with `reply`.
fn run(job: &Job, reply: &mut dyn FnMut(&Request) -> Answer) -> Seen {
    let Job::Stop {
        name,
        channel,
        thread,
        by,
    } = job
    else {
        panic!("not a stop: {job:?}");
    };
    let seen = RefCell::default();
    with_fx(&seen, reply, |fx| stop(fx, name, (channel, thread), by));
    seen.into_inner()
}

fn steer_ok(_: &Request) -> Answer {
    Ok(Response::ok("steer", Some(json!({}))))
}

#[test]
fn the_requester_pressing_stop_queues_an_interrupt_that_goes_out_as_the_requester() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = shown(&db);
    let name = row.box_name.as_str();
    let job = click(&press(name, "U1", "env-1"), &db, 1).unwrap().unwrap();
    let queued = Job::Stop {
        name: row.box_name.clone(),
        channel: "C1".into(),
        thread: "10.5".into(),
        by: "U1".into(),
    };
    assert_eq!(job, queued);
    let seen = run(&job, &mut steer_ok);
    let want = Request::Steer(SteerReq {
        target: row.box_name.clone().into(),
        text: None,
        menu: None,
        interrupt: true,
    });
    assert_eq!(seen.calls, [want]);
    assert_eq!(seen.users, ["U1"]);
    assert!(seen.replies.is_empty(), "{:?}", seen.replies);
}

#[test]
fn anyone_else_gets_a_refusal_and_nothing_is_queued_to_send() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let name = shown(&db).box_name;
    let job = click(&press(name.as_str(), "U2", "env-1"), &db, 1).unwrap();
    let refusal = Job::Ephemeral {
        channel: "C1".into(),
        user: "U2".into(),
        thread: "10.5".into(),
        text: "Only <@U1> can stop this Box.".into(),
    };
    assert_eq!(job, Some(refusal));
}

#[test]
fn a_redelivered_press_queues_one_stop() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let name = shown(&db).box_name;
    let press = press(name.as_str(), "U1", "env-1");
    assert!(matches!(
        click(&press, &db, 1).unwrap(),
        Some(Job::Stop { .. })
    ));
    assert_eq!(click(&press, &db, 2).unwrap(), None);
}

#[test]
fn a_press_on_a_message_that_is_no_box_status_or_names_another_box_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let name = shown(&db).box_name;
    let mut elsewhere = press(name.as_str(), "U1", "env-1");
    elsewhere.message_ts = "other".into();
    assert_eq!(click(&elsewhere, &db, 1).unwrap(), None);
    let wrong = press("another-box", "U1", "env-2");
    assert_eq!(click(&wrong, &db, 1).unwrap(), None);
}

#[test]
fn a_press_on_a_closing_box_says_so_and_stops_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let name = shown(&db).box_name;
    db.mark_closing(&name, false).unwrap();
    let job = click(&press(name.as_str(), "U1", "env-1"), &db, 1).unwrap();
    let Some(Job::Ephemeral { user, text, .. }) = job else {
        panic!("expected a refusal, got {job:?}");
    };
    assert_eq!(user, "U1");
    assert!(text.contains("closing or closed"), "{text}");
}

#[test]
fn an_interrupt_the_box_refuses_or_that_fails_is_posted_in_the_thread() {
    let job = Job::Stop {
        name: moor_core::name::BoxName::parse_allow_bare("fix-bug").unwrap(),
        channel: "C1".into(),
        thread: "10.5".into(),
        by: "U1".into(),
    };
    let seen = run(&job, &mut |_| refused("steer", "not running"));
    let said: Vec<&str> = seen.replies.iter().map(|(_, _, t)| t.as_str()).collect();
    assert_eq!(said, ["Box `fix-bug` refused the interrupt: not running"]);
    let seen = run(&job, &mut |_| hung_up());
    assert!(seen.replies[0]
        .2
        .starts_with("The interrupt may not have reached Box `fix-bug`"));
}
