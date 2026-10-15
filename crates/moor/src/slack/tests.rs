//! Tests for the mention worker: dedupe, allow-list, start and thread binding.

use std::cell::RefCell;
use std::sync::Arc;

use moor_core::request::StartReq;
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
    }
}

/// What the effects saw.
#[derive(Default)]
struct Seen {
    starts: Vec<StartReq>,
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
    let mut start = |req: &StartReq| {
        seen.borrow_mut().starts.push(req.clone());
        fail.map_or(Ok(()), |m| Err(m.to_owned()))
    };
    let mut reply = |c: &str, t: &str, text: &str| {
        seen.borrow_mut()
            .replies
            .push((c.to_owned(), t.to_owned(), text.to_owned()));
    };
    let log = |line: &str| seen.borrow_mut().logs.push(line.to_owned());
    handle(m, store, &allow, &mut start, &mut reply, &log);
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
    assert_eq!(seen.starts.len(), 1);
    let req = seen.starts.first().unwrap();
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
    assert_eq!(seen.starts.len(), 1);
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
    assert_eq!(seen.starts.len(), 1);
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
    assert!(seen.starts.is_empty());
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
    assert_eq!(seen.starts.len(), 1);
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
    assert!(seen.starts.is_empty());
    assert!(seen.replies.is_empty());
    assert_eq!(bound(&store, "C9", "10.5"), None);
}
