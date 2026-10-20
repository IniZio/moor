//! Tests for the worker: dedupe, allow-list, start, the thread binding and recovery from a
//! lost controller connection.
#![allow(
    clippy::panic,
    clippy::unnecessary_wraps,
    reason = "test helpers build controller answers"
)]

use std::cell::RefCell;
use std::io;

use moor_client::ClientError;
use moor_core::controller::FrameErrorKind;
use moor_core::name::BoxId;
use moor_core::request::Request;
use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::json;

use super::{handle, resume, Fx};
use crate::command::Allow;
use crate::db::{Db, SlackThread, ThreadState};
use crate::socket::Mention;

fn db(dir: &std::path::Path) -> Db {
    Db::open(&dir.join("slack.db")).unwrap()
}

fn mention(event_id: &str, user: &str, channel: &str, ts: &str, thread: Option<&str>) -> Mention {
    Mention {
        event_id: event_id.into(),
        channel: channel.into(),
        user: user.into(),
        text: "<@UBOT> start api fix the flaky test".into(),
        ts: ts.into(),
        thread_ts: thread.map(str::to_owned),
        reply: false,
    }
}

/// What the effects saw.
#[derive(Default)]
struct Seen {
    calls: Vec<Request>,
    replies: Vec<(String, String, String)>,
    waits: usize,
}

/// A controller answer for one request.
type Answer = Result<Response, ClientError>;

/// The reply of a `start` or `show` that names Box 7 on branch `moor/<name>`.
fn named(cmd: &str, name: &str) -> Answer {
    let value = json!({"box": {"id": 7, "name": name, "branch": format!("moor/{name}")}});
    Ok(Response::ok(cmd, Some(value)))
}

fn refused(cmd: &str, message: &str) -> Answer {
    let error = ErrorView {
        kind: ErrorKind::Failed,
        message: message.to_owned(),
        hints: Vec::new(),
        context: Vec::new(),
    };
    Ok(Response::err(cmd, error))
}

fn hung_up() -> Answer {
    Err(ClientError::Io(io::Error::from(
        io::ErrorKind::ConnectionReset,
    )))
}

/// Runs a mention whose first `start` is lost; later requests are answered by `answer`.
fn run_after_lost_start(db: &Db, seen: &RefCell<Seen>, answer: &mut dyn FnMut(&Request) -> Answer) {
    let m = mention("Ev1", "U1", "C1", "10.5", None);
    let mut first = true;
    run_with(db, &m, seen, &mut |req| {
        if std::mem::take(&mut first) {
            hung_up()
        } else {
            answer(req)
        }
    });
}

fn still_starting(db: &Db) -> bool {
    bound(db, "C1", "10.5") == Some(("slack-10-5".to_owned(), ThreadState::Starting))
}

/// Runs `step` with effects that answer each request through `answer`.
fn with_fx(
    seen: &RefCell<Seen>,
    answer: &mut dyn FnMut(&Request) -> Answer,
    step: impl FnOnce(&mut Fx<'_>),
) {
    let mut call = |req: Request| {
        let reply = answer(&req);
        seen.borrow_mut().calls.push(req);
        reply
    };
    let mut reply = |c: &str, t: &str, text: &str| {
        seen.borrow_mut()
            .replies
            .push((c.to_owned(), t.to_owned(), text.to_owned()));
    };
    let wait = |_| seen.borrow_mut().waits += 1;
    let mut fx = Fx {
        call: &mut call,
        reply: &mut reply,
        log: &|_| {},
        wait: &wait,
        now: || 1_000,
    };
    step(&mut fx);
}

/// Runs `handle` for `m` in channel `C1` for user `U1`, answering each request with `answer`.
fn run_with(
    db: &Db,
    m: &Mention,
    seen: &RefCell<Seen>,
    answer: &mut dyn FnMut(&Request) -> Answer,
) {
    let channels = vec!["C1".to_owned()];
    let users = vec!["U1".to_owned()];
    let allow = Allow {
        channels: &channels,
        users: &users,
    };
    with_fx(seen, answer, |fx| handle(m, db, &allow, fx));
}

/// Runs `handle` for `m`; `fail` makes the controller refuse the start with that error.
fn run(db: &Db, m: &Mention, fail: Option<&str>, seen: &RefCell<Seen>) {
    run_with(db, m, seen, &mut |req| match (req, fail) {
        (Request::Start(s), None) => named("start", s.name.as_str()),
        (_, Some(message)) => refused("start", message),
        _ => refused("show", "no Box"),
    });
}

/// The command name of each request, in order.
fn cmds(seen: &Seen) -> Vec<&'static str> {
    seen.calls
        .iter()
        .map(|r| match r {
            Request::Start(_) => "start",
            Request::Show(_) => "show",
            _ => "other",
        })
        .collect()
}

fn bound(db: &Db, channel: &str, thread: &str) -> Option<(String, ThreadState)> {
    db.thread(channel, thread)
        .unwrap()
        .map(|t| (t.box_name.as_str().to_owned(), t.state))
}

fn bound_to_7(name: &str) -> Option<(String, ThreadState)> {
    let state = ThreadState::Bound {
        box_id: BoxId::new(7),
        branch: Some(format!("moor/{name}")),
    };
    Some((name.to_owned(), state))
}

#[test]
fn allowed_mention_starts_binds_and_replies() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    run(&db, &mention("Ev1", "U1", "C1", "10.5", None), None, &seen);
    let seen = seen.into_inner();
    let Some(Request::Start(req)) = seen.calls.first() else {
        panic!("no start: {:?}", cmds(&seen));
    };
    assert_eq!(seen.calls.len(), 1);
    assert_eq!(req.name.as_str(), "slack-10-5");
    assert_eq!(req.repo.as_deref(), Some("api"));
    assert_eq!(req.goal.as_deref(), Some("fix the flaky test"));
    assert_eq!(bound(&db, "C1", "10.5"), bound_to_7("slack-10-5"));
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(seen.replies.first().unwrap().1, "10.5");
    assert!(seen.replies.first().unwrap().2.contains("slack-10-5"));
}

#[test]
fn redelivered_event_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    let m = mention("Ev1", "U1", "C1", "10.5", None);
    run(&db, &m, None, &seen);
    run(&db, &m, None, &seen);
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), ["start"]);
    assert_eq!(seen.replies.len(), 1);
}

#[test]
fn second_mention_in_thread_names_the_bound_box() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    run(&db, &mention("Ev1", "U1", "C1", "10.5", None), None, &seen);
    let again = mention("Ev2", "U1", "C1", "11.0", Some("10.5"));
    run(&db, &again, None, &seen);
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), ["start"]);
    assert_eq!(seen.replies.len(), 2);
    assert_eq!(seen.replies.get(1).unwrap().1, "10.5");
    assert!(seen.replies.get(1).unwrap().2.contains("slack-10-5"));
}

#[test]
fn disallowed_user_is_refused_without_starting() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    run(&db, &mention("Ev1", "U9", "C1", "10.5", None), None, &seen);
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty());
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(seen.replies.first().unwrap().1, "10.5");
    assert!(seen.replies.first().unwrap().2.contains("not allowed"));
    assert_eq!(bound(&db, "C1", "10.5"), None);
}

#[test]
fn failed_start_replies_and_binds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    let m = mention("Ev1", "U1", "C1", "10.5", None);
    run(&db, &m, Some("unknown repo"), &seen);
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), ["start"]);
    assert_eq!(seen.replies.len(), 1);
    assert!(seen.replies.first().unwrap().2.contains("unknown repo"));
    assert_eq!(bound(&db, "C1", "10.5"), None);
}

#[test]
fn disallowed_channel_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    run(&db, &mention("Ev1", "U1", "C9", "10.5", None), None, &seen);
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty());
    assert!(seen.replies.is_empty());
    assert_eq!(bound(&db, "C9", "10.5"), None);
}

#[test]
fn lost_start_then_show_finds_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    let m = mention("Ev1", "U1", "C1", "10.5", None);
    run_with(&db, &m, &seen, &mut |req| match req {
        Request::Start(_) => hung_up(),
        _ => named("show", "slack-10-5"),
    });
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), ["start", "show"]);
    assert_eq!(seen.waits, 1);
    assert_eq!(bound(&db, "C1", "10.5"), bound_to_7("slack-10-5"));
    assert_eq!(seen.replies.len(), 1);
    assert!(seen.replies.first().unwrap().2.starts_with("Started Box"));
}

#[test]
fn lost_start_before_the_box_exists_is_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    let m = mention("Ev1", "U1", "C1", "10.5", None);
    let mut starts = 0;
    run_with(&db, &m, &seen, &mut |req| match req {
        Request::Start(s) => {
            starts += 1;
            if starts == 1 {
                hung_up()
            } else {
                named("start", s.name.as_str())
            }
        }
        _ => refused("show", "no Box slack-10-5"),
    });
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), ["start", "show", "start"]);
    let Some(Request::Start(resent)) = seen.calls.get(2) else {
        panic!("no resend");
    };
    assert_eq!(resent.goal.as_deref(), Some("fix the flaky test"));
    assert_eq!(bound(&db, "C1", "10.5"), bound_to_7("slack-10-5"));
    assert_eq!(seen.replies.len(), 1);
}

#[test]
fn starting_row_is_settled_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = SlackThread {
        channel: "C1".into(),
        thread_ts: "10.5".into(),
        box_name: moor_core::BoxName::parse_allow_bare("slack-10-5").unwrap(),
        repo: "api".into(),
        goal: "fix the flaky test".into(),
        owner: "U1".into(),
        state: ThreadState::Starting,
    };
    db.insert_starting(&row, 1).unwrap();
    let seen = RefCell::default();
    with_fx(&seen, &mut |_| named("show", "slack-10-5"), |fx| {
        resume(&db, fx);
    });
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), ["show"]);
    assert_eq!(bound(&db, "C1", "10.5"), bound_to_7("slack-10-5"));
    assert_eq!(seen.replies.first().map(|r| r.1.as_str()), Some("10.5"));
}

#[test]
fn refused_hello_after_a_lost_start_leaves_the_row_starting() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    run_after_lost_start(&db, &seen, &mut |_| {
        Err(ClientError::Frame {
            kind: FrameErrorKind::UnsupportedProtocol,
            message: "client speaks protocol 7, controller speaks 8".into(),
        })
    });
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), ["start", "show"]);
    assert!(still_starting(&db));
    assert!(seen.replies.is_empty());
}

#[test]
fn show_failure_other_than_no_box_leaves_the_row_starting() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    run_after_lost_start(&db, &seen, &mut |_| refused("show", "store unavailable"));
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), ["start", "show"]);
    assert!(still_starting(&db));
    assert!(seen.replies.is_empty());
}

#[test]
fn rejected_box_found_by_show_is_a_failed_start() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    run_after_lost_start(&db, &seen, &mut |_| {
        let value =
            json!({"box": {"id": 7, "lifecycle": "rejected", "state_reason": "no credential"}});
        Ok(Response::ok("show", Some(value)))
    });
    let seen = seen.into_inner();
    assert_eq!(bound(&db, "C1", "10.5"), None);
    let text = &seen.replies.first().unwrap().2;
    assert_eq!(text, "Could not start Box `slack-10-5`: no credential");
}

#[test]
fn start_lost_three_times_is_left_for_the_next_startup() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let seen = RefCell::default();
    let m = mention("Ev1", "U1", "C1", "10.5", None);
    run_with(&db, &m, &seen, &mut |req| match req {
        Request::Start(_) => hung_up(),
        _ => refused("show", "no Box slack-10-5"),
    });
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), ["start", "show", "start", "show", "start"]);
    assert!(still_starting(&db));
    assert!(seen.replies.is_empty());
}
