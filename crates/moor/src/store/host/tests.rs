//! Tests for Sessions that run on the host.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionState;
use moor_core::domain::task::TaskInput;
use moor_core::BoxId;

use crate::store::tests::{new_task, open};
use crate::store::{Store, StoreError};

type Row = (Option<i64>, Option<String>, String, Option<i64>, i64);

fn row(store: &Store, id: i64) -> Row {
    store
        .reader()
        .unwrap()
        .query_row(
            "SELECT box, workdir, state, ended_at, ordinal FROM sessions WHERE id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
}

fn agent_cols(store: &Store, id: i64) -> (Option<String>, Option<String>) {
    store
        .reader()
        .unwrap()
        .query_row(
            "SELECT agent, agent_session_id FROM sessions WHERE id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

fn with_host(name: &str) -> (tempfile::TempDir, Store, BoxId, i64) {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, name);
    let host = store.open_host_session("/w", None, None).unwrap();
    (dir, store, id, host)
}

#[test]
fn host_session_is_working_with_no_box() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = store
        .open_host_session("/w", Some("claude"), Some("abc"))
        .unwrap();
    let (box_id, workdir, state, ended_at, ordinal) = row(&store, id);
    assert_eq!(box_id, None);
    assert_eq!(workdir.as_deref(), Some("/w"));
    assert_eq!((state.as_str(), ended_at, ordinal), ("working", None, 1));
    assert_eq!(
        agent_cols(&store, id),
        (Some("claude".to_owned()), Some("abc".to_owned()))
    );
}

#[test]
fn host_session_agent_is_optional() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = store.open_host_session("/w", None, None).unwrap();
    assert_eq!(agent_cols(&store, id), (None, None));
}

fn event_count(store: &Store) -> i64 {
    store
        .reader()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn host_session_records_no_events() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    new_task(&store, "acme/1");
    let before = event_count(&store);
    store.open_host_session("/w", None, None).unwrap();
    assert_eq!(event_count(&store), before);
}

#[test]
fn empty_workdir_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let err = store.open_host_session("", None, None).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

#[test]
fn two_host_sessions_both_insert() {
    let (_dir, store, _id, first) = with_host("acme/1");
    let second = store.open_host_session("/w", None, None).unwrap();
    assert_ne!(first, second);
    assert_eq!(row(&store, first).4, 1);
    assert_eq!(row(&store, second).4, 1);
}

#[test]
fn box_reads_never_see_host_sessions() {
    let (_dir, store, id, host) = with_host("acme/1");
    let sessions = store.box_sessions(id).unwrap();
    assert!(sessions.iter().all(|s| s.id != host));
    assert_eq!(sessions.len(), 1);
    assert!(store.task(id).unwrap().is_some());
}

#[test]
fn ending_and_reopening_a_box_session_leaves_host_alone() {
    let (_dir, store, id, host) = with_host("acme/1");
    let before = row(&store, host);
    assert!(store.end_current_session(id, "test", None).unwrap());
    let next = store.open_session(id).unwrap();
    assert_eq!(next.ordinal, 2);
    assert_eq!(store.box_sessions(id).unwrap().len(), 2);
    assert_eq!(row(&store, host), before);
}

#[test]
fn ending_a_task_leaves_host_alone() {
    let (_dir, store, id, host) = with_host("acme/1");
    let before = row(&store, host);
    let reject = TaskInput::Rejected {
        reason: "no".into(),
    };
    store.apply(Decision::new(id).task_input(reject)).unwrap();
    assert_eq!(row(&store, host), before);
    assert_eq!(row(&store, host).2, "working");
}

fn invalid_msg(err: StoreError) -> String {
    let StoreError::Invalid(msg) = err else {
        return format!("not Invalid: {err:?}");
    };
    msg
}

#[test]
fn update_sets_agent_session_id_and_keeps_agent() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = store.open_host_session("/w", Some("claude"), None).unwrap();
    let got = store.update_host_session(id, None, Some("abc")).unwrap();
    assert_eq!(got.agent.as_deref(), Some("claude"));
    assert_eq!(got.agent_session_id.as_deref(), Some("abc"));
    assert_eq!(store.host_session(id).unwrap(), Some(got));
}

#[test]
fn unknown_id_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let err = store.update_host_session(99, None, None).unwrap_err();
    assert_eq!(invalid_msg(err), "no host Session 99");
    let err = store.end_host_session(99).unwrap_err();
    assert_eq!(invalid_msg(err), "no host Session 99");
}

#[test]
fn box_session_id_is_refused() {
    let (_dir, store, id, _host) = with_host("acme/1");
    let box_session = store.box_sessions(id).unwrap()[0].id;
    assert_eq!(store.host_session(box_session).unwrap(), None);
    let err = store
        .update_host_session(box_session, Some("x"), None)
        .unwrap_err();
    assert_eq!(invalid_msg(err), format!("no host Session {box_session}"));
    let err = store.end_host_session(box_session).unwrap_err();
    assert_eq!(invalid_msg(err), format!("no host Session {box_session}"));
}

#[test]
fn end_sets_state_and_ended_at() {
    let (_dir, store, _id, host) = with_host("acme/1");
    let got = store.end_host_session(host).unwrap();
    assert_eq!(got.state, SessionState::Ended);
    assert!(got.ended_at.is_some());
    assert_eq!(store.host_session(host).unwrap(), Some(got));
    assert_eq!(row(&store, host).2, "ended");
}

#[test]
fn update_after_end_is_refused() {
    let (_dir, store, _id, host) = with_host("acme/1");
    store.end_host_session(host).unwrap();
    let err = store
        .update_host_session(host, Some("x"), None)
        .unwrap_err();
    assert_eq!(invalid_msg(err), format!("host Session {host} has ended"));
}

#[test]
fn end_twice_is_refused() {
    let (_dir, store, _id, host) = with_host("acme/1");
    store.end_host_session(host).unwrap();
    let err = store.end_host_session(host).unwrap_err();
    assert_eq!(invalid_msg(err), format!("host Session {host} has ended"));
}
