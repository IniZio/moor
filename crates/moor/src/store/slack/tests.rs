//! Tests for Slack event dedup and thread binding.

use moor_core::TaskName;

use crate::store::tests::open;
use crate::store::Store;

fn name(s: &str) -> TaskName {
    TaskName::parse(s).unwrap()
}

#[test]
fn event_is_new_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert!(store.slack_event_once("Ev1").unwrap());
    assert!(!store.slack_event_once("Ev1").unwrap());
    assert!(store.slack_event_once("Ev2").unwrap());
}

#[test]
fn thread_binding_first_wins() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert_eq!(store.slack_thread("C1", "1.1").unwrap(), None);
    assert!(store
        .bind_slack_thread("C1", "1.1", &name("first"))
        .unwrap());
    assert_eq!(
        store.slack_thread("C1", "1.1").unwrap(),
        Some(name("first"))
    );
    assert!(!store
        .bind_slack_thread("C1", "1.1", &name("second"))
        .unwrap());
    assert_eq!(
        store.slack_thread("C1", "1.1").unwrap(),
        Some(name("first"))
    );
    assert_eq!(store.slack_thread("C1", "2.2").unwrap(), None);
}

#[test]
fn existing_v1_db_gains_tables() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    store
        .writer()
        .unwrap()
        .execute_batch("DROP TABLE slack_events; DROP TABLE slack_threads;")
        .unwrap();
    drop(store);
    let path = dir.path().join("state").join("moor.db");
    let store = Store::open(&path, clock).unwrap();
    assert!(store.slack_event_once("Ev1").unwrap());
    assert!(store.bind_slack_thread("C1", "1.1", &name("t")).unwrap());
}
