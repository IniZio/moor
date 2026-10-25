//! Answering a prompt without the coordinator: what a click or a bare number queues, and how
//! the worker records the answer around the `steer` that sends it.
#![allow(clippy::panic, reason = "test code")]

use std::cell::RefCell;

use moor_core::name::BoxId;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::{db, hung_up, recorded_start, refused, with_fx, Answer, Seen};
use crate::db::{Db, Posted, PromptState, SlackBox};
use crate::socket::{Click, Mention, Press};
use crate::worker::answer::{answer, click, digit};
use crate::worker::Job;

/// A bound Box in thread `10.5` with one open prompt offering `a` and `b`; returns the Box
/// row and the prompt id.
fn asked(db: &Db) -> (SlackBox, i64) {
    let row = recorded_start(db);
    db.bind(&row.box_name, BoxId::new(7), None).unwrap();
    let posted = Posted {
        box_name: &row.box_name,
        event_id: 5,
        message_ts: Some("m5"),
        question: "Which?",
        options: &["a".to_owned(), "b".to_owned()],
    };
    db.insert_prompt(&posted, 1).unwrap();
    let id = db.prompt_by_message("C1", "m5").unwrap().unwrap().id;
    (row, id)
}

/// Runs the worker's answer of `choice` to prompt `id` as `U1`, answering each request with
/// `reply`.
fn answered(db: &Db, id: i64, choice: u32, reply: &mut dyn FnMut(&Request) -> Answer) -> Seen {
    let seen = RefCell::default();
    with_fx(&seen, reply, |fx| answer(db, fx, id, choice, "U1"));
    seen.into_inner()
}

fn steer_ok(_: &Request) -> Answer {
    Ok(Response::ok("steer", Some(json!({}))))
}

fn state(db: &Db, id: i64) -> PromptState {
    db.prompt(id).unwrap().unwrap().state
}

fn texts(seen: &Seen) -> Vec<&str> {
    seen.replies.iter().map(|(_, _, t)| t.as_str()).collect()
}

#[test]
fn a_box_that_moves_on_before_the_steer_reply_keeps_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (row, id) = asked(&db);
    let feed = Db::open(&dir.path().join("slack.db")).unwrap();
    let seen = answered(&db, id, 2, &mut |req| {
        feed.close_prompt(id, "working").unwrap();
        steer_ok(req)
    });
    assert_eq!(seen.calls.len(), 1);
    assert!(seen.replies.is_empty(), "{:?}", seen.replies);
    let answer = PromptState::Answered {
        label: "2. b".into(),
        by: "U1".into(),
    };
    assert_eq!(state(&db, id), answer);
    assert_eq!(db.unshown_answers(&row.box_name).unwrap().len(), 1);
}

#[test]
fn a_steer_the_box_refuses_or_that_fails_opens_the_prompt_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (row, id) = asked(&db);
    let seen = answered(&db, id, 1, &mut |_| refused("steer", "not waiting"));
    assert_eq!(state(&db, id), PromptState::Open);
    let name = row.box_name.as_str();
    assert_eq!(
        texts(&seen),
        [format!("Box `{name}` refused the answer: not waiting")]
    );
    let seen = answered(&db, id, 1, &mut |_| hung_up());
    assert_eq!(state(&db, id), PromptState::Open);
    assert!(texts(&seen)[0].starts_with("The answer may not have reached"));
    assert!(db.unshown_answers(&row.box_name).unwrap().is_empty());
}

#[test]
fn the_worker_checks_again_that_the_prompt_is_open_and_steers_nothing_when_it_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (row, id) = asked(&db);
    db.close_prompt(id, "working").unwrap();
    let seen = answered(&db, id, 1, &mut steer_ok);
    assert!(seen.calls.is_empty());
    let name = row.box_name.as_str();
    assert_eq!(
        texts(&seen),
        [format!(
            "This prompt is no longer open: {name} is now working."
        )]
    );
}

#[test]
fn an_answer_to_an_answered_prompt_quotes_the_answer_escaped() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (_, id) = asked(&db);
    db.answer_prompt(id, "1. <!channel>", "U1").unwrap();
    let seen = answered(&db, id, 2, &mut steer_ok);
    assert!(seen.calls.is_empty());
    assert_eq!(
        texts(&seen),
        ["This prompt was already answered: 1. &lt;!channel&gt;."]
    );
}

#[test]
fn a_redelivered_click_queues_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (_, id) = asked(&db);
    let press = Click {
        envelope_id: "env-1".into(),
        channel: "C1".into(),
        user: "U1".into(),
        message_ts: "m5".into(),
        thread_ts: Some("10.5".into()),
        press: Press::Answer(2),
    };
    let first = click(&press, &db, 1).unwrap();
    let queued = Job::Answer {
        prompt: id,
        choice: 2,
        by: "U1".into(),
    };
    assert_eq!(first, Some(queued));
    assert_eq!(click(&press, &db, 2).unwrap(), None);
}

#[test]
fn a_number_the_prompt_does_not_offer_goes_on_to_the_coordinator() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (_, id) = asked(&db);
    let said = |text: &str| Mention {
        event_id: format!("Ev-{text}"),
        channel: "C1".into(),
        user: "U1".into(),
        text: text.into(),
        ts: "10.9".into(),
        thread_ts: Some("10.5".into()),
        reply: true,
    };
    assert_eq!(digit(&db, &said("3"), "10.5").unwrap(), None);
    let picked = Job::Answer {
        prompt: id,
        choice: 2,
        by: "U1".into(),
    };
    assert_eq!(digit(&db, &said("2"), "10.5").unwrap(), Some(picked));
}
