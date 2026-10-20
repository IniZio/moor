//! A post lands before its `slack.db` write, so a crash or a failed write between them
//! posts again later instead of losing the reply.
#![allow(clippy::panic, reason = "test code; a panic stands in for a crash")]

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc;
use std::time::Duration;

use moor_core::request::Request;

use super::{
    bound_to_7, cmds, db, named, recorded_start, refused, state, with_fx, with_hooks, Answer, Seen,
};
use crate::db::{Db, SlackThread, ThreadState};
use crate::worker::{serve, settle, Attempt, Job};

/// Makes the first attempt at `row` with a worker that crashes right after its post lands,
/// before it writes `slack.db`.
fn crash_after_post(
    db: &Db,
    row: &SlackThread,
    answer: &mut dyn FnMut(&Request) -> Answer,
) -> Seen {
    let seen = RefCell::default();
    let crashed = std::panic::catch_unwind(AssertUnwindSafe(|| {
        with_hooks(
            &seen,
            answer,
            &|| panic!("crash after the post"),
            &|_| {},
            |fx| {
                settle(db, fx, row, Attempt::First);
            },
        );
    }));
    assert!(crashed.is_err(), "the worker never posted");
    seen.into_inner()
}

/// Restarts the worker on the database in `dir` and lets it settle what the crash left.
fn restart(dir: &std::path::Path, answer: &mut dyn FnMut(&Request) -> Answer) -> (Db, Seen) {
    let db = db(dir);
    let left = db.starting().unwrap();
    let seen = RefCell::default();
    let (_, jobs) = mpsc::channel();
    with_fx(&seen, answer, |fx| serve(&jobs, &db, left, fx));
    (db, seen.into_inner())
}

#[test]
fn crash_between_the_started_post_and_the_bind_posts_again_without_a_second_start() {
    let dir = tempfile::tempdir().unwrap();
    let first = db(dir.path());
    let row = recorded_start(&first);
    let before = crash_after_post(&first, &row, &mut |req| match req {
        Request::Start(s) => named("start", s.name.as_str()),
        _ => refused("show", "unexpected"),
    });
    drop(first);
    let (db, after) = restart(dir.path(), &mut |req| match req {
        Request::Show(_) => named("show", "slack-10-5"),
        _ => refused("start", "Box slack-10-5 started twice"),
    });
    assert_eq!(cmds(&after), [("show", "slack-10-5".into())]);
    assert_eq!(before.replies.len(), 1);
    assert_eq!(after.replies, before.replies);
    assert_eq!(state(&db, "C1", "10.5"), bound_to_7("slack-10-5"));
}

#[test]
fn crash_between_the_failure_post_and_the_delete_posts_again() {
    let dir = tempfile::tempdir().unwrap();
    let first = db(dir.path());
    let row = recorded_start(&first);
    let mut unknown_repo = |req: &Request| match req {
        Request::Start(_) => refused("start", "unknown repo"),
        _ => refused("show", "no Box slack-10-5"),
    };
    let before = crash_after_post(&first, &row, &mut unknown_repo);
    drop(first);
    let (db, after) = restart(dir.path(), &mut unknown_repo);
    assert_eq!(before.replies.len(), 1);
    assert_eq!(after.replies, before.replies);
    assert_eq!(state(&db, "C1", "10.5"), None);
}

/// Serves the recorded start while `trigger` fails its first `slack.db` write; the log line
/// about that failure drops the trigger. Returns what the effects saw and the row's state
/// when the failure was logged.
fn serve_with_failing_write(
    dir: &std::path::Path,
    trigger: &str,
    answer: &mut dyn FnMut(&Request) -> Answer,
) -> (Db, Seen, Option<ThreadState>) {
    let db = db(dir);
    let row = recorded_start(&db);
    let other = rusqlite::Connection::open(dir.join("slack.db")).unwrap();
    other
        .execute_batch(&format!(
            "CREATE TRIGGER fail {trigger} ON slack_threads \
             BEGIN SELECT RAISE(ABORT, 'disk full'); END;"
        ))
        .unwrap();
    let watcher = self::db(dir);
    let at_failure = RefCell::new(None);
    let log = |line: &str| {
        if line.starts_with("slack.db: ") {
            *at_failure.borrow_mut() = Some(state(&watcher, "C1", "10.5"));
            other.execute_batch("DROP TRIGGER IF EXISTS fail;").unwrap();
        }
    };
    let (tx, jobs) = mpsc::channel();
    let feeder = std::thread::spawn(move || {
        tx.send(Job::Start(row)).unwrap();
        std::thread::sleep(Duration::from_secs(1));
    });
    let seen = RefCell::default();
    with_hooks(&seen, answer, &|| {}, &log, |fx| {
        serve(&jobs, &db, Vec::new(), fx);
    });
    feeder.join().unwrap();
    let at_failure = at_failure.into_inner().expect("no slack.db failure logged");
    (db, seen.into_inner(), at_failure)
}

#[test]
fn failed_bind_leaves_the_row_starting_and_the_next_retry_binds_it() {
    let dir = tempfile::tempdir().unwrap();
    let (db, seen, at_failure) =
        serve_with_failing_write(dir.path(), "BEFORE UPDATE", &mut |req| match req {
            Request::Start(s) => named("start", s.name.as_str()),
            Request::Show(s) => named("show", s.box_name.as_ref().unwrap().as_str()),
            _ => refused("other", "unexpected"),
        });
    assert_eq!(at_failure, Some(ThreadState::Starting));
    assert_eq!(
        cmds(&seen),
        [
            ("start", "slack-10-5".into()),
            ("show", "slack-10-5".into())
        ]
    );
    assert_eq!(seen.replies.len(), 2);
    assert_eq!(state(&db, "C1", "10.5"), bound_to_7("slack-10-5"));
}

#[test]
fn failed_delete_leaves_the_row_starting_and_the_next_retry_deletes_it() {
    let dir = tempfile::tempdir().unwrap();
    let (db, seen, at_failure) =
        serve_with_failing_write(dir.path(), "BEFORE DELETE", &mut |req| match req {
            Request::Start(_) => refused("start", "unknown repo"),
            _ => refused("show", "no Box slack-10-5"),
        });
    assert_eq!(at_failure, Some(ThreadState::Starting));
    assert_eq!(
        cmds(&seen),
        [
            ("start", "slack-10-5".into()),
            ("show", "slack-10-5".into()),
            ("start", "slack-10-5".into()),
        ]
    );
    assert!(seen.replies.iter().all(|r| r.2.contains("unknown repo")));
    assert_eq!(state(&db, "C1", "10.5"), None);
}
