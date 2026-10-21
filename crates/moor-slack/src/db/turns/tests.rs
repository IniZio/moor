//! Tests for coordinator turn rows: one open turn per thread, the items a turn takes, and the
//! history the next turn reads.

use super::super::{Db, InboxItem, ItemKind, SlackThread};
use super::End;
use crate::coordinator::context::{Said, Speaker};

/// A database with thread `1.0` of channel `C1`, started by `U1`.
fn thread() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("slack.db")).unwrap();
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        repo: String::new(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    (dir, db)
}

/// Queues a `kind` item from `U1` with `text`, keyed by `source`.
fn queue(db: &Db, kind: ItemKind, text: &str, source: &str) {
    let item = InboxItem {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        kind,
        author: "U1".into(),
        text: text.into(),
        source: source.into(),
    };
    db.queue(&item, 2).unwrap();
}

fn said(text: &str) -> Said {
    Said {
        speaker: Speaker::Person("U1".into()),
        text: text.into(),
    }
}

#[test]
fn one_turn_takes_every_waiting_message_and_no_close() {
    let (_dir, db) = thread();
    queue(&db, ItemKind::Message, "one", "Ev1");
    queue(&db, ItemKind::Close, "close", "Ev2");
    queue(&db, ItemKind::Message, "two", "Ev3");
    let turn = db.take_turn(5).unwrap().unwrap();
    assert_eq!((turn.channel.as_str(), turn.owner.as_str()), ("C1", "U1"));
    let items: Vec<Said> = db
        .turn_items(turn.id)
        .unwrap()
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    assert_eq!(items, [said("one"), said("two")]);
    queue(&db, ItemKind::Message, "three", "Ev4");
    assert_eq!(db.take_turn(6).unwrap(), None, "one open turn per thread");
    db.store_reply(turn.id, End::Replied, "done", Some(10), None)
        .unwrap();
    db.mark_posted(turn.id, 7).unwrap();
    let next = db.take_turn(8).unwrap().unwrap();
    let items: Vec<Said> = db
        .turn_items(next.id)
        .unwrap()
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    assert_eq!(items, [said("three")]);
}

#[test]
fn a_thread_with_only_a_close_waiting_takes_no_turn() {
    let (_dir, db) = thread();
    queue(&db, ItemKind::Close, "close", "Ev1");
    assert_eq!(db.take_turn(5).unwrap(), None);
}

#[test]
fn history_lists_each_earlier_turns_items_then_its_reply() {
    let (_dir, db) = thread();
    queue(&db, ItemKind::Message, "first", "Ev1");
    let one = db.take_turn(5).unwrap().unwrap();
    db.store_reply(one.id, End::Replied, "answer one", None, None)
        .unwrap();
    db.mark_posted(one.id, 6).unwrap();
    queue(&db, ItemKind::Message, "second", "Ev2");
    let two = db.take_turn(7).unwrap().unwrap();
    db.store_reply(
        two.id,
        End::Failed,
        "The coordinator stopped: x.",
        None,
        None,
    )
    .unwrap();
    db.mark_posted(two.id, 8).unwrap();
    queue(&db, ItemKind::Message, "third", "Ev3");
    let three = db.take_turn(9).unwrap().unwrap();
    let history: Vec<Said> = db
        .history("C1", "1.0", three.id)
        .unwrap()
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    let reply = Said {
        speaker: Speaker::Coordinator,
        text: "answer one".into(),
    };
    assert_eq!(history, [said("first"), reply, said("second")]);
    let first = db.first_request("C1", "1.0").unwrap().map(|(_, t)| t);
    assert_eq!(first.as_deref(), Some("first"));
}
