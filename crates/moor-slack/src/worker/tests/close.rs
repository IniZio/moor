//! Messages in a kept thread and the close path: a starter's message, `close` included, is
//! queued for the coordinator, and the worker settles a Box that `close_box` marked
//! `closing`, including after a lost controller connection.
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
use crate::command::BoxEvent;
use crate::db::{BoxState, Db, InboxItem, ItemKind, SlackBox};
use crate::socket::Mention;
use crate::worker::{serve, settle, Attempt};

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

/// Thread `10.5` with its Box `slack-10-5-1` bound to id 7.
fn bound_thread(db: &Db) {
    let row = recorded_start(db);
    db.bind(&row.box_name, BoxId::new(7), None).unwrap();
}

/// Marks the thread's bound Box `closing`, as `close_box` does, and returns its row.
fn recorded_close(db: &Db) -> SlackBox {
    let row = db.thread_boxes("C1", "10.5").unwrap().remove(0);
    db.mark_closing(&row.box_name).unwrap();
    let row = db.thread_boxes("C1", "10.5").unwrap().remove(0);
    assert_eq!(Some(row.state.clone()), closing());
    row
}

/// An inbox item of thread `10.5`.
fn item(kind: ItemKind, author: &str, text: &str, source: &str) -> InboxItem {
    InboxItem {
        channel: "C1".into(),
        thread_ts: "10.5".into(),
        kind,
        author: author.into(),
        text: text.into(),
        message_ts: None,
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
    let value = json!({"box": {"id": 7, "name": "slack-10-5-1", "lifecycle": lifecycle}});
    Ok(Response::ok("show", Some(value)))
}

/// What the effects of one closing attempt saw, plus the replies' texts.
fn texts(seen: &Seen) -> Vec<&str> {
    seen.replies.iter().map(|r| r.2.as_str()).collect()
}

/// The Box state `slack-10-5-1` is closing from.
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
    let texts: Vec<_> = db
        .inbox("C1", "10.5")
        .unwrap()
        .into_iter()
        .map(|i| i.text)
        .collect();
    assert_eq!(
        texts,
        ["fix the flaky test", "use tokio"],
        "the opening message, then this one"
    );
}

#[test]
fn a_close_reply_is_queued_for_the_coordinator_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    bound_thread(&db);
    assert_eq!(record_one(&db, &reply("Ev2", "U1", "close")), None);
    let want = item(ItemKind::Message, "U1", "close", "Ev2");
    assert_eq!(db.inbox("C1", "10.5").unwrap().last(), Some(&want));
    let bound = BoxState::Bound {
        box_id: BoxId::new(7),
        branch: None,
    };
    assert_eq!(
        state(&db, "C1", "10.5"),
        Some(bound),
        "the front closes nothing"
    );
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
    assert_eq!(
        (req.target.to_string(), req.force),
        ("slack-10-5-1".to_owned(), false)
    );
    assert_eq!(texts(&seen), ["Closed Box `slack-10-5-1`."]);
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
    for answer in [shown("closed"), refused("show", "no Box slack-10-5-1")] {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        bound_thread(&db);
        let row = recorded_close(&db);
        let mut answer = Some(answer);
        let seen = attempt(&db, &row, Attempt::Again, &mut |_| answer.take().unwrap());
        assert_eq!(cmds(&seen), [("show", "slack-10-5-1".into())]);
        assert_eq!(texts(&seen), ["Closed Box `slack-10-5-1`."]);
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
    assert_eq!(texts(&seen), ["Closed Box `slack-10-5-1`."]);
}

#[test]
fn a_mention_in_a_kept_thread_is_queued_like_a_reply() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    bound_thread(&db);
    let mut m = reply("Ev2", "U1", "<@UBOT> start api again");
    m.reply = false;
    assert_eq!(record_one(&db, &m), None);
    let items = db.inbox("C1", "10.5").unwrap();
    assert_eq!(
        items.last().map(|i| i.text.as_str()),
        Some("start api again")
    );
}

#[test]
fn a_settled_close_leaves_the_box_gone_or_bound_again() {
    for (answer, text, gone) in [
        (Ok(Response::ok("close", None)), "Closed Box", true),
        (
            refused("close", "unpushed work"),
            "Could not close Box",
            false,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        bound_thread(&db);
        let row = recorded_close(&db);
        let mut answer = Some(answer);
        let seen = attempt(&db, &row, Attempt::First, &mut |_| answer.take().unwrap());
        assert!(texts(&seen)[0].starts_with(text), "{:?}", texts(&seen));
        let after = state(&db, "C1", "10.5").unwrap();
        assert_eq!(matches!(after, BoxState::Gone { .. }), gone, "{after:?}");
        assert_eq!(matches!(after, BoxState::Bound { .. }), !gone, "{after:?}");
        let noticed = db.has_notice(&row, BoxEvent::Closed, None).unwrap();
        assert_eq!(noticed, gone, "only a closed Box queues the closed notice");
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
    assert_eq!(texts(&before), ["Closed Box `slack-10-5-1`."]);
    drop(first);
    let db = db(dir.path());
    assert_eq!(
        state(&db, "C1", "10.5"),
        closing(),
        "the write ran before the post"
    );
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
}
