//! Tests for the mention worker: dedupe, allow-list, start and thread binding.

use std::cell::RefCell;
use std::sync::Arc;

use moor_core::request::{CloseReq, Request, StartReq, SteerReq};
use moor_core::seam::clock::Ts;
use moor_core::TaskName;

use super::command::Allow;
use super::handle;
use super::socket::Mention;
use crate::store::Store;
use crate::testing::clock::FakeClock;

fn store(dir: &std::path::Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
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
    logs: Vec<String>,
}

/// Runs `handle` for `m`; `fail` makes the start effect return that error.
fn run(store: &Store, m: &Mention, fail: Option<&str>, seen: &RefCell<Seen>) {
    let channels = vec!["C1".to_owned()];
    let users = vec!["U1".to_owned()];
    let allow = Allow {
        channels: &channels,
        users: &users,
    };
    let mut call = |req: Request| {
        seen.borrow_mut().calls.push(req);
        fail.map_or(Ok(()), |m| Err(m.to_owned()))
    };
    let mut reply = |c: &str, t: &str, text: &str| {
        seen.borrow_mut()
            .replies
            .push((c.to_owned(), t.to_owned(), text.to_owned()));
    };
    let log = |line: &str| seen.borrow_mut().logs.push(line.to_owned());
    handle(m, store, &allow, &mut call, &mut reply, &log);
}

/// The Start requests among the calls.
fn starts(seen: &Seen) -> Vec<&StartReq> {
    seen.calls
        .iter()
        .filter_map(|r| match r {
            Request::Start(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn bound(store: &Store, channel: &str, thread: &str) -> Option<TaskName> {
    store.slack_thread(channel, thread).unwrap()
}

#[test]
fn allowed_mention_starts_binds_and_replies() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U1", "C1", "10.5", None),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    assert_eq!(starts(&seen).len(), 1);
    let all = starts(&seen);
    let req = all.first().unwrap();
    assert_eq!(req.name.as_str(), "slack-10-5");
    assert_eq!(req.repo.as_deref(), Some("api"));
    assert_eq!(req.goal.as_deref(), Some("fix the flaky test"));
    assert_eq!(bound(&store, "C1", "10.5").unwrap().as_str(), "slack-10-5");
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(seen.replies.first().unwrap().1, "10.5");
    assert!(seen.replies.first().unwrap().2.contains("slack-10-5"));
}

#[test]
fn redelivered_event_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    let m = mention("Ev1", "U1", "C1", "10.5", None);
    run(&store, &m, None, &seen);
    run(&store, &m, None, &seen);
    let seen = seen.into_inner();
    assert_eq!(starts(&seen).len(), 1);
    assert_eq!(seen.replies.len(), 1);
}

#[test]
fn second_mention_in_thread_names_the_bound_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U1", "C1", "10.5", None),
        None,
        &seen,
    );
    let again = mention("Ev2", "U1", "C1", "11.0", Some("10.5"));
    run(&store, &again, None, &seen);
    let seen = seen.into_inner();
    assert_eq!(starts(&seen).len(), 1);
    assert_eq!(seen.replies.len(), 2);
    assert_eq!(seen.replies.get(1).unwrap().1, "10.5");
    assert!(seen.replies.get(1).unwrap().2.contains("slack-10-5"));
}

#[test]
fn disallowed_user_is_refused_without_starting() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U9", "C1", "10.5", None),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    assert!(starts(&seen).is_empty());
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(seen.replies.first().unwrap().1, "10.5");
    assert!(seen.replies.first().unwrap().2.contains("not allowed"));
    assert_eq!(bound(&store, "C1", "10.5"), None);
}

#[test]
fn failed_start_replies_and_binds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U1", "C1", "10.5", None),
        Some("unknown repo"),
        &seen,
    );
    let seen = seen.into_inner();
    assert_eq!(starts(&seen).len(), 1);
    assert_eq!(seen.replies.len(), 1);
    assert!(seen.replies.first().unwrap().2.contains("unknown repo"));
    assert_eq!(bound(&store, "C1", "10.5"), None);
}

#[test]
fn disallowed_channel_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U1", "C9", "10.5", None),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    assert!(starts(&seen).is_empty());
    assert!(seen.replies.is_empty());
    assert_eq!(bound(&store, "C9", "10.5"), None);
}

fn reply_to(event_id: &str, user: &str, channel: &str, text: &str) -> Mention {
    Mention {
        text: text.into(),
        reply: true,
        ..mention(event_id, user, channel, "12.0", Some("10.5"))
    }
}

/// A store with a Task bound to thread `10.5` in `C1`, owned by `U1`.
fn owned(dir: &std::path::Path) -> Store {
    let store = store(dir);
    let name = TaskName::parse("slack-10-5").unwrap();
    assert!(store.bind_slack_thread("C1", "10.5", &name, "U1").unwrap());
    store
}

#[test]
fn owner_reply_steers_the_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &reply_to("Ev2", "U1", "C1", "try the other test"),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    let Some(Request::Steer(SteerReq { task, text, menu })) = seen.calls.first() else {
        unreachable!("expected steer, got {:?}", seen.calls);
    };
    assert_eq!(task.as_str(), "slack-10-5");
    assert_eq!(text.as_deref(), Some("try the other test"));
    assert_eq!(*menu, None);
    assert!(seen.replies.is_empty());
}

#[test]
fn other_user_reply_is_refused_with_owner_mention() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &reply_to("Ev2", "U2", "C1", "do something else"),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty());
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(seen.replies.first().unwrap().1, "10.5");
    assert!(seen.replies.first().unwrap().2.contains("<@U1>"));
}

#[test]
fn reply_in_unbound_thread_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(&store, &reply_to("Ev2", "U1", "C1", "hello"), None, &seen);
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty());
    assert!(seen.replies.is_empty());
}

#[test]
fn reply_in_disallowed_channel_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let name = TaskName::parse("slack-10-5").unwrap();
    store.bind_slack_thread("C9", "10.5", &name, "U1").unwrap();
    let seen = RefCell::default();
    run(&store, &reply_to("Ev2", "U1", "C9", "hello"), None, &seen);
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty());
    assert!(seen.replies.is_empty());
}

#[test]
fn owner_close_closes_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    run(&store, &reply_to("Ev2", "U1", "C1", "close"), None, &seen);
    let seen = seen.into_inner();
    let Some(Request::Close(CloseReq {
        task,
        force,
        trust_moor_yaml,
    })) = seen.calls.first()
    else {
        unreachable!("expected close, got {:?}", seen.calls);
    };
    assert_eq!(task.as_str(), "slack-10-5");
    assert!(!force && !trust_moor_yaml);
}

#[test]
fn close_refusal_is_posted_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &reply_to("Ev2", "U1", "C1", "close"),
        Some("unmerged work in slack-10-5"),
        &seen,
    );
    let seen = seen.into_inner();
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(
        seen.replies.first().unwrap().2,
        "unmerged work in slack-10-5"
    );
}

#[test]
fn redelivered_reply_is_handled_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    let m = reply_to("Ev2", "U1", "C1", "keep going");
    run(&store, &m, None, &seen);
    run(&store, &m, None, &seen);
    assert_eq!(seen.into_inner().calls.len(), 1);
}
