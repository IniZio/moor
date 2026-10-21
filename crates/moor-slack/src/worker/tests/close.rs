//! Messages in a kept thread: what the starter's messages and `close` record before the
//! ack, and how the worker settles a close, including after a lost controller connection.
#![allow(clippy::panic, reason = "test code")]

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc;

use moor_core::name::BoxId;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::{
    cmds, db, record_one, recorded_start, refused, refused_hello, state, try_record, with_fx,
    with_hooks, Answer, Seen,
};
use crate::db::{BoxState, Db, InboxItem, ItemKind, SlackBox};
use crate::socket::Mention;
use crate::worker::{serve, settle, Attempt, Job};

/// A plain reply in thread `10.5` of channel `C1`.
fn reply(event_id: &str, user: &str, text: &str) -> Mention {
    Mention {
        event_id: event_id.into(),
        channel: "C1".into(),
        user: user.into(),
        text: text.into(),
        ts: "11.0".into(),
        thread_ts: Some("10.5".into()),
        reply: true,
    }
}

/// Thread `10.5` with its Box `slack-10-5` bound to id 7.
fn bound_thread(db: &Db) {
    let row = recorded_start(db);
    db.bind(&row.box_name, BoxId::new(7), None).unwrap();
}

/// What `close` from `U1` records, which must be a close job.
fn recorded_close(db: &Db) -> SlackBox {
    match record_one(db, &reply("Ev2", "U1", "close")) {
        Some(Job::Close(row)) => row,
        other => panic!("no close: {other:?}"),
    }
}

/// An inbox item of thread `10.5`.
fn item(kind: ItemKind, author: &str, text: &str, source: &str) -> InboxItem {
    InboxItem {
        channel: "C1".into(),
        thread_ts: "10.5".into(),
        kind,
        author: author.into(),
        text: text.into(),
        source: source.into(),
    }
}

/// Makes one `attempt` at `row`, answering each request with `answer`.
fn attempt(
    db: &Db,
    row: &SlackBox,
    attempt: Attempt,
    answer: &mut dyn FnMut(&Request) -> Answer,
) -> Seen {
    let seen = RefCell::default();
    with_fx(&seen, answer, |fx| {
        settle(db, fx, row, attempt);
    });
    seen.into_inner()
}

/// A `show` reply for Box 7 in `lifecycle`.
fn shown(lifecycle: &str) -> Answer {
    let value = json!({"box": {"id": 7, "name": "slack-10-5", "lifecycle": lifecycle}});
    Ok(Response::ok("show", Some(value)))
}

/// What the effects of one closing attempt saw, plus the replies' texts.
fn texts(seen: &Seen) -> Vec<&str> {
    seen.replies.iter().map(|r| r.2.as_str()).collect()
}

/// The Box state `slack-10-5` is closing from.
fn closing() -> Option<BoxState> {
    Some(BoxState::Closing {
        box_id: BoxId::new(7),
        branch: None,
    })
}

#[test]
fn a_reply_in_a_thread_the_front_does_not_keep_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    assert_eq!(record_one(&db, &reply("Ev2", "U1", "close")), None);
    assert_eq!(db.inbox("C1", "10.5").unwrap(), []);
}

#[test]
fn a_failed_record_queues_nothing_so_the_resent_message_is_queued() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    bound_thread(&db);
    let other = rusqlite::Connection::open(dir.path().join("slack.db")).unwrap();
    other
        .execute_batch(
            "CREATE TRIGGER no_items BEFORE INSERT ON slack_inbox \
             BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
        )
        .unwrap();
    let m = reply("Ev2", "U1", "use tokio");
    assert!(try_record(&db, &m).is_err());
    other.execute_batch("DROP TRIGGER no_items;").unwrap();
    assert_eq!(try_record(&db, &m).unwrap(), None);
    assert_eq!(db.inbox("C1", "10.5").unwrap().len(), 1);
}

#[test]
fn close_from_the_starter_marks_the_box_closing_with_its_item() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    bound_thread(&db);
    let row = recorded_close(&db);
    assert_eq!(Some(row.state), closing());
    assert_eq!(state(&db, "C1", "10.5"), closing());
    let want = item(ItemKind::Close, "U1", "close", "Ev2");
    assert_eq!(db.inbox("C1", "10.5").unwrap(), [want]);
}

#[test]
fn close_of_a_box_still_starting_or_closing_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    recorded_start(&db);
    let Some(Job::Reply { text, .. }) = record_one(&db, &reply("Ev5", "U1", "close")) else {
        panic!("no reply");
    };
    assert!(text.contains("is still starting"), "{text}");
    assert_eq!(state(&db, "C1", "10.5"), Some(BoxState::Starting));
    let started = db.thread_boxes("C1", "10.5").unwrap().remove(0);
    db.bind(&started.box_name, BoxId::new(7), None).unwrap();
    recorded_close(&db);
    let Some(Job::Reply { text, .. }) = record_one(&db, &reply("Ev3", "U1", "close")) else {
        panic!("no reply");
    };
    assert!(text.contains("is already closing"), "{text}");
    assert_eq!(db.inbox("C1", "10.5").unwrap().len(), 1);
}

#[test]
fn a_close_is_sent_without_force_and_the_box_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    bound_thread(&db);
    let row = recorded_close(&db);
    let seen = attempt(&db, &row, Attempt::First, &mut |_| {
        Ok(Response::ok("close", None))
    });
    let Some(Request::Close(req)) = seen.calls.first() else {
        panic!("no close: {:?}", seen.calls);
    };
    assert_eq!((req.box_name.as_str(), req.force), ("slack-10-5", false));
    assert_eq!(texts(&seen), ["Closed Box `slack-10-5`."]);
    let gone = BoxState::Gone {
        box_id: BoxId::new(7),
        branch: None,
    };
    assert_eq!(state(&db, "C1", "10.5"), Some(gone));
}

#[test]
fn a_refused_hello_on_the_first_close_is_posted() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    bound_thread(&db);
    let row = recorded_close(&db);
    let seen = attempt(&db, &row, Attempt::First, &mut |_| refused_hello());
    assert!(
        texts(&seen)[0].contains("controller speaks 8"),
        "{:?}",
        texts(&seen)
    );
}

#[test]
fn show_finds_the_box_a_lost_close_closed() {
    for answer in [shown("closed"), refused("show", "no Box slack-10-5")] {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        bound_thread(&db);
        let row = recorded_close(&db);
        let mut answer = Some(answer);
        let seen = attempt(&db, &row, Attempt::Again, &mut |_| answer.take().unwrap());
        assert_eq!(cmds(&seen), [("show", "slack-10-5".into())]);
        assert_eq!(texts(&seen), ["Closed Box `slack-10-5`."]);
    }
}

#[test]
fn a_box_still_open_after_a_lost_close_is_closed_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    bound_thread(&db);
    let row = recorded_close(&db);
    let seen = attempt(&db, &row, Attempt::Again, &mut |req| match req {
        Request::Show(_) => shown("ready"),
        _ => refused("close", "Box is already closed"),
    });
    assert_eq!(seen.calls.len(), 2, "a show, then a close");
    assert_eq!(texts(&seen), ["Closed Box `slack-10-5`."]);
}

#[test]
fn a_start_command_in_a_kept_thread_is_not_queued() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    bound_thread(&db);
    let mut m = reply("Ev2", "U1", "<@UBOT> start api again");
    m.reply = false;
    assert!(matches!(record_one(&db, &m), Some(Job::Reply { .. })));
    assert_eq!(db.inbox("C1", "10.5").unwrap(), []);
}

/// `done_at` of every `close` item in the `slack.db` under `dir`, read on its own connection.
fn close_done_at(dir: &std::path::Path) -> Vec<Option<i64>> {
    let conn = rusqlite::Connection::open(dir.join("slack.db")).unwrap();
    let mut stmt = conn
        .prepare("SELECT done_at FROM slack_inbox WHERE kind = 'close' ORDER BY id")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.map(Result::unwrap).collect()
}

#[test]
fn a_settled_close_marks_its_item_done_whether_the_box_closed_or_not() {
    for (answer, text) in [
        (Ok(Response::ok("close", None)), "Closed Box"),
        (refused("close", "unpushed work"), "Could not close Box"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        bound_thread(&db);
        let row = recorded_close(&db);
        assert_eq!(close_done_at(dir.path()), [None], "queued, not yet done");
        let mut answer = Some(answer);
        let seen = attempt(&db, &row, Attempt::First, &mut |_| answer.take().unwrap());
        assert!(texts(&seen)[0].starts_with(text), "{:?}", texts(&seen));
        let done = close_done_at(dir.path());
        assert!(
            matches!(done[..], [Some(_)]),
            "close item not done: {done:?}"
        );
    }
}

#[test]
fn a_crash_after_the_closed_post_leaves_the_close_to_settle_and_post_again_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let first = db(dir.path());
    bound_thread(&first);
    let row = recorded_close(&first);
    let seen = RefCell::default();
    let crashed = std::panic::catch_unwind(AssertUnwindSafe(|| {
        with_hooks(
            &seen,
            &mut |_| Ok(Response::ok("close", None)),
            &|| panic!("crash after the post"),
            &|_| {},
            |fx| {
                settle(&first, fx, &row, Attempt::First);
            },
        );
    }));
    assert!(crashed.is_err(), "the worker never posted");
    let before = seen.into_inner();
    assert_eq!(texts(&before), ["Closed Box `slack-10-5`."]);
    drop(first);
    let db = db(dir.path());
    assert_eq!(
        state(&db, "C1", "10.5"),
        closing(),
        "the write ran before the post"
    );
    assert_eq!(close_done_at(dir.path()), [None]);
    let left = db.closing().unwrap();
    let seen = RefCell::default();
    let (_, jobs) = mpsc::channel();
    let mut answer = |req: &Request| match req {
        Request::Show(_) => shown("closed"),
        _ => refused("close", "Box is already closed"),
    };
    with_fx(&seen, &mut answer, |fx| serve(&jobs, &db, left, fx));
    let after = seen.into_inner();
    assert_eq!(after.replies, before.replies);
    assert!(matches!(close_done_at(dir.path())[..], [Some(_)]));
}
