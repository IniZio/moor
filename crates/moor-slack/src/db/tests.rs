//! Tests for `slack.db`: event dedupe and the thread rows.

use std::path::Path;

use moor_core::name::{BoxId, BoxName};

use super::{Db, SlackThread, ThreadState, SCHEMA_VERSION};

fn open(dir: &Path) -> Db {
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

/// Writes a `slack.db` at `version` with one table, as an older or newer build would leave it.
fn write_at_version(path: &Path, version: i64) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("CREATE TABLE old_rows (id INTEGER);")
        .unwrap();
    conn.pragma_update(None, "user_version", version).unwrap();
}

/// Every file in `dir` with its bytes.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&p).unwrap())
        })
        .collect();
    files.sort();
    files
}

#[test]
fn newer_schema_is_refused() {
    for found in [1, SCHEMA_VERSION - 1, SCHEMA_VERSION + 1] {
        let dir = tempfile::tempdir().unwrap();
        write_at_version(&dir.path().join("slack.db"), found);
        let err = Db::open(&dir.path().join("slack.db")).err().unwrap();
        let want = format!(
            "slack.db is schema {found} and this build needs {SCHEMA_VERSION}. Start with a fresh slack.db."
        );
        assert_eq!(err, want);
    }
}

#[test]
fn refused_file_is_left_as_found() {
    for version in [0, 1, 99] {
        let dir = tempfile::tempdir().unwrap();
        write_at_version(&dir.path().join("slack.db"), version);
        let before = snapshot(dir.path());
        assert!(Db::open(&dir.path().join("slack.db")).is_err());
        assert_eq!(snapshot(dir.path()), before, "version {version}");
    }
}

#[test]
fn new_file_gets_the_schema_once_and_opens_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slack.db");
    let first = Db::open(&path).unwrap();
    let second = Db::open(&path).unwrap();
    assert!(first.event_once("Ev1", 1).unwrap());
    assert!(!second.event_once("Ev1", 2).unwrap());
}
