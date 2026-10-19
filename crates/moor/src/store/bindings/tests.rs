//! Tests for binding a front's conversation key to a Session.

#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::BoxId;

use crate::store::tests::{new_task, open};
use crate::store::{Store, StoreError};

fn box_session(store: &Store, id: BoxId) -> i64 {
    store
        .reader()
        .unwrap()
        .query_row("SELECT id FROM sessions WHERE box = ?", [id.get()], |r| {
            r.get(0)
        })
        .unwrap()
}

#[test]
fn host_session_binds_and_resolves_back() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let session = store.open_host_session("/w", None, None).unwrap();
    assert!(store.bind_session("cli", "k1", session, "U1").unwrap());
    let found = store.binding("cli", "k1").unwrap().unwrap();
    assert_eq!(found.front, "cli");
    assert_eq!(found.key, "k1");
    assert_eq!(found.session, session);
    assert_eq!(found.task, None);
    assert_eq!(found.owner, "U1");
}

#[test]
fn box_session_binds_with_its_task_name() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_task(&store, "acme/1");
    let session = box_session(&store, id);
    assert!(store.bind_session("cli", "k1", session, "U1").unwrap());
    let found = store.binding("cli", "k1").unwrap().unwrap();
    assert_eq!(found.session, session);
    assert_eq!(found.task.unwrap().as_str(), "acme/1");
}

#[test]
fn unbound_key_resolves_to_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert!(store.binding("cli", "missing").unwrap().is_none());
}

#[test]
fn second_bind_on_a_key_keeps_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let first = store.open_host_session("/a", None, None).unwrap();
    let second = store.open_host_session("/b", None, None).unwrap();
    assert!(store.bind_session("cli", "k1", first, "U1").unwrap());
    assert!(!store.bind_session("cli", "k1", second, "U2").unwrap());
    let found = store.binding("cli", "k1").unwrap().unwrap();
    assert_eq!(found.session, first);
    assert_eq!(found.owner, "U1");
}

#[test]
fn unknown_session_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let err = store.bind_session("cli", "k1", 9999, "U1").unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    assert!(store.binding("cli", "k1").unwrap().is_none());
}

#[test]
fn ended_session_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let session = store.open_host_session("/w", None, None).unwrap();
    store
        .writer()
        .unwrap()
        .execute(
            "UPDATE sessions SET state = 'ended', ended_at = 1 WHERE id = ?",
            [session],
        )
        .unwrap();
    let err = store.bind_session("cli", "k1", session, "U1").unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    assert!(store.binding("cli", "k1").unwrap().is_none());
}

#[test]
fn bindings_list_host_and_box_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let host = store.open_host_session("/w", None, None).unwrap();
    let id = new_task(&store, "acme/1");
    let boxed = box_session(&store, id);
    store.bind_session("cli", "a", host, "U1").unwrap();
    store.bind_session("cli", "b", boxed, "U2").unwrap();
    let all = store.bindings("cli").unwrap();
    let keys: Vec<&str> = all.iter().map(|b| b.key.as_str()).collect();
    assert_eq!(keys, ["a", "b"]);
    assert_eq!(all[0].task, None);
    assert!(all[1].task.is_some());
}

#[test]
fn bindings_only_include_the_asked_front() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let session = store.open_host_session("/w", None, None).unwrap();
    store.bind_session("cli", "a", session, "U1").unwrap();
    store.bind_session("web", "b", session, "U1").unwrap();
    let cli = store.bindings("cli").unwrap();
    assert_eq!(cli.len(), 1);
    assert_eq!(cli[0].key, "a");
    assert!(store.bindings("none").unwrap().is_empty());
}
