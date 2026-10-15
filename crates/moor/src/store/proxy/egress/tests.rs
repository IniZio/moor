//! Tests for recording proxy audit events.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::{TaskId, TaskName};
use serde_json::json;

use crate::store::{Store, StoreError};
use crate::testing::clock::FakeClock;

/// A store in a temp dir with one task.
fn setup(dir: &std::path::Path) -> (Store, TaskId) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.join("moor.db"), clock).unwrap();
    let nt = NewTask {
        name: TaskName::parse("t").unwrap(),
        repo: "r".into(),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().task;
    (store, id)
}

#[test]
fn egress_refused_is_appended() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let e = Event::EgressRefused {
        host: "evil.example".into(),
        rule: Some("gh".into()),
        method: None,
        category: None,
    };
    store.record_proxy_event(id, &e).unwrap();
    let rows = store.events(id, 100).unwrap();
    let row = rows.iter().find(|r| r.kind == "egress_refused").unwrap();
    assert_eq!(
        row.payload,
        json!({"host": "evil.example", "rule": "gh", "method": null, "category": null})
    );
    assert_eq!(row.ts, Ts::from_millis(1_000));
}

#[test]
fn credential_withheld_is_appended() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let e = Event::CredentialWithheld {
        host: "api.github.com".into(),
        rule: "gh".into(),
        method: "GET".into(),
        shape: Some("GET /repos/{repo}/pulls/*".into()),
    };
    store.record_proxy_event(id, &e).unwrap();
    let rows = store.events(id, 100).unwrap();
    let row = rows
        .iter()
        .find(|r| r.kind == "credential_withheld")
        .unwrap();
    assert_eq!(
        row.payload,
        json!({"host": "api.github.com", "rule": "gh", "method": "GET", "shape": "GET /repos/{repo}/pulls/*"})
    );
}

#[test]
fn credential_failure_is_appended() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let e = Event::ProxyCredentialFailed { rule: "gh".into() };
    store.record_proxy_event(id, &e).unwrap();
    let rows = store.events(id, 100).unwrap();
    let row = rows
        .iter()
        .find(|r| r.kind == "proxy_credential_failed")
        .unwrap();
    assert_eq!(row.payload, json!({"rule": "gh"}));
}

#[test]
fn other_events_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let before = store.events(id, 100).unwrap().len();
    let e = Event::ProxyStarted {
        box_port: 1,
        host_port: 2,
        pid: 3,
        handle: "h".into(),
    };
    let err = store.record_proxy_event(id, &e).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    assert_eq!(store.events(id, 100).unwrap().len(), before);
}

#[test]
fn unknown_task_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let e = Event::ProxyCredentialFailed { rule: "gh".into() };
    let err = store
        .record_proxy_event(TaskId::new(id.get() + 100), &e)
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)), "{err:?}");
}
