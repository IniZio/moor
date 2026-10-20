//! Tests for `slack.db`: event dedupe and the thread rows.

use moor_core::name::{BoxId, BoxName};

use super::{Db, SlackThread, ThreadState};

fn open(dir: &std::path::Path) -> Db {
    Db::open(&dir.join("slack.db")).unwrap()
}

fn starting(thread_ts: &str, name: &str, owner: &str) -> SlackThread {
    SlackThread {
        channel: "C1".into(),
        thread_ts: thread_ts.into(),
        box_name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "api".into(),
        goal: "fix it".into(),
        owner: owner.into(),
        state: ThreadState::Starting,
    }
}

#[test]
fn event_is_new_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    assert!(db.event_once("Ev1", 1).unwrap());
    assert!(!db.event_once("Ev1", 2).unwrap());
    assert!(db.event_once("Ev2", 3).unwrap());
    drop(db);
    assert!(!open(dir.path()).event_once("Ev1", 4).unwrap());
}

#[test]
fn thread_binding_first_wins() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    assert_eq!(db.thread("C1", "1.1").unwrap(), None);
    let first = starting("1.1", "first", "U1");
    db.insert_starting(&first, 1).unwrap();
    assert!(db
        .insert_starting(&starting("1.1", "second", "U2"), 2)
        .is_err());
    assert_eq!(db.thread("C1", "1.1").unwrap(), Some(first));
    assert_eq!(db.thread("C1", "2.2").unwrap(), None);
}

#[test]
fn owner_kept_from_first_bind() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    db.insert_starting(&starting("1.1", "first", "U1"), 1)
        .unwrap();
    db.bind("C1", "1.1", BoxId::new(7), Some("moor/first"))
        .unwrap();
    db.bind("C1", "1.1", BoxId::new(8), None).unwrap();
    let row = db.thread("C1", "1.1").unwrap().unwrap();
    assert_eq!(row.owner, "U1");
    let want = ThreadState::Bound {
        box_id: BoxId::new(7),
        branch: Some("moor/first".into()),
    };
    assert_eq!(row.state, want);
}

#[test]
fn starting_rows_are_listed_oldest_first_and_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    db.insert_starting(&starting("2.2", "b", "U1"), 5).unwrap();
    db.insert_starting(&starting("1.1", "a", "U1"), 9).unwrap();
    db.insert_starting(&starting("3.3", "c", "U1"), 1).unwrap();
    db.bind("C1", "3.3", BoxId::new(3), None).unwrap();
    db.forget_starting("C1", "3.3").unwrap();
    let names: Vec<String> = db
        .starting()
        .unwrap()
        .iter()
        .map(|t| t.box_name.as_str().to_owned())
        .collect();
    assert_eq!(names, ["b", "a"]);
    db.forget_starting("C1", "2.2").unwrap();
    assert_eq!(db.thread("C1", "2.2").unwrap(), None);
    drop(db);
    let db = open(dir.path());
    assert_eq!(db.starting().unwrap().len(), 1);
    assert!(db.thread("C1", "3.3").unwrap().is_some());
}

#[test]
fn newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    db.conn.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    let err = Db::open(&dir.path().join("slack.db")).err().unwrap();
    assert!(err.contains("newer"), "{err}");
}
