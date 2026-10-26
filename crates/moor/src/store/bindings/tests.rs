//! Tests for binding a front's conversation key to a Session.

#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::BoxId;

use crate::store::tests::{new_box, open};
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
    assert_eq!(found.box_name, None);
    assert_eq!(found.owner, "U1");
}

#[test]
fn box_session_binds_with_its_box_name() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_box(&store, "acme/1");
    let session = box_session(&store, id);
    assert!(store.bind_session("cli", "k1", session, "U1").unwrap());
    let found = store.binding("cli", "k1").unwrap().unwrap();
    assert_eq!(found.session, session);
    assert_eq!(found.box_name.unwrap().as_str(), "acme/1");
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
        .execute(
            "UPDATE sessions SET state = 'ended', ended_at = 1 WHERE id = ?",
            [session],
        )
        .unwrap();
    let err = store.bind_session("cli", "k1", session, "U1").unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    assert!(store.binding("cli", "k1").unwrap().is_none());
}

fn end(store: &Store, session: i64) {
    store.end_host_session(session).unwrap();
}

#[test]
fn a_second_session_binds_once_the_first_has_ended() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let first = store.open_host_session("/a", None, None).unwrap();
    assert!(store.bind_session("cli", "k1", first, "U1").unwrap());
    end(&store, first);
    let second = store.open_host_session("/b", None, None).unwrap();
    assert!(store.bind_session("cli", "k1", second, "U2").unwrap());
    let found = store.binding("cli", "k1").unwrap().unwrap();
    assert_eq!(found.session, second);
    assert_eq!(found.owner, "U2");
}

#[test]
fn every_session_a_key_had_keeps_its_binding() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let first = store.open_host_session("/a", None, None).unwrap();
    store.bind_session("cli", "k1", first, "U1").unwrap();
    end(&store, first);
    let second = store.open_host_session("/b", None, None).unwrap();
    store.bind_session("cli", "k1", second, "U2").unwrap();
    for (session, owner) in [(first, "U1"), (second, "U2")] {
        let found = store.session_binding(session).unwrap().unwrap();
        assert_eq!((found.front.as_str(), found.key.as_str()), ("cli", "k1"));
        assert_eq!((found.session, found.owner.as_str()), (session, owner));
    }
    let unbound = store.open_host_session("/c", None, None).unwrap();
    assert!(store.session_binding(unbound).unwrap().is_none());
}

#[test]
fn with_no_live_session_the_newest_binding_is_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let first = store.open_host_session("/a", None, None).unwrap();
    store.bind_session("cli", "k1", first, "U1").unwrap();
    end(&store, first);
    let second = store.open_host_session("/b", None, None).unwrap();
    store.bind_session("cli", "k1", second, "U2").unwrap();
    end(&store, second);
    assert_eq!(store.binding("cli", "k1").unwrap().unwrap().session, second);
}

#[test]
fn binding_the_same_session_again_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let session = store.open_host_session("/a", None, None).unwrap();
    assert!(store.bind_session("cli", "k1", session, "U1").unwrap());
    assert!(!store.bind_session("cli", "k1", session, "U2").unwrap());
    let found = store.binding("cli", "k1").unwrap().unwrap();
    assert_eq!((found.session, found.owner.as_str()), (session, "U1"));
    let rows: i64 = store
        .reader()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM bindings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}
