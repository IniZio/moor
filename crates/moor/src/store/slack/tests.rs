//! Tests for Slack event dedup and thread binding.

use moor_core::seam::clock::Clock;
use moor_core::TaskName;

use crate::store::tests::open;
use crate::store::{SlackThread, Store};

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
        .bind_slack_thread("C1", "1.1", &name("first"), "U1")
        .unwrap());
    assert_eq!(
        store.slack_thread("C1", "1.1").unwrap(),
        Some(name("first"))
    );
    assert!(!store
        .bind_slack_thread("C1", "1.1", &name("second"), "U2")
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
    assert!(store
        .bind_slack_thread("C1", "1.1", &name("t"), "U1")
        .unwrap());
}

#[test]
fn owner_kept_from_first_bind() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert_eq!(store.slack_thread_owner("C1", "1.1").unwrap(), None);
    store
        .bind_slack_thread("C1", "1.1", &name("first"), "U1")
        .unwrap();
    store
        .bind_slack_thread("C1", "1.1", &name("second"), "U2")
        .unwrap();
    assert_eq!(
        store.slack_thread_owner("C1", "1.1").unwrap().as_deref(),
        Some("U1")
    );
}

#[test]
fn status_ts_set_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store
        .bind_slack_thread("C1", "1.1", &name("t"), "U1")
        .unwrap();
    assert_eq!(
        store.slack_threads().unwrap().first().unwrap().status_ts,
        None
    );
    store.set_slack_status_ts("C1", "1.1", "9.9").unwrap();
    assert_eq!(
        store
            .slack_threads()
            .unwrap()
            .first()
            .unwrap()
            .status_ts
            .as_deref(),
        Some("9.9")
    );
}

#[test]
fn threads_listed_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    assert!(store.slack_threads().unwrap().is_empty());
    store
        .bind_slack_thread("C1", "2.2", &name("b"), "U2")
        .unwrap();
    clock.sleep(std::time::Duration::from_secs(1));
    store
        .bind_slack_thread("C1", "1.1", &name("a"), "U1")
        .unwrap();
    let threads = store.slack_threads().unwrap();
    assert_eq!(
        threads,
        vec![
            SlackThread {
                channel: "C1".into(),
                thread_ts: "2.2".into(),
                task: name("b"),
                owner: "U2".into(),
                status_ts: None,
            },
            SlackThread {
                channel: "C1".into(),
                thread_ts: "1.1".into(),
                task: name("a"),
                owner: "U1".into(),
                status_ts: None,
            },
        ]
    );
}
