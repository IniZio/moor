//! Tests for reading Sessions by id and by parent.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::domain::session_state::SessionState;
use moor_core::{BoxId, BoxName};

use crate::store::tests::{new_task, open};
use crate::store::Store;

fn end_current(store: &Store, id: BoxId) {
    store
        .writer()
        .unwrap()
        .execute(
            "UPDATE sessions SET state = 'ended' WHERE box = ?",
            [id.get()],
        )
        .unwrap();
}

fn first_session(store: &Store, id: BoxId) -> i64 {
    store.box_sessions(id).unwrap()[0].id
}

#[test]
fn host_session_row_has_no_task_and_keeps_its_fields() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = store
        .open_host_session("/w", Some("a"), Some("s1"))
        .unwrap();
    let row = store.session_row(id).unwrap().unwrap();
    assert_eq!(row.task, None);
    assert_eq!(row.workdir.as_deref(), Some("/w"));
    assert_eq!(row.agent.as_deref(), Some("a"));
    assert_eq!(row.agent_session_id.as_deref(), Some("s1"));
    assert_eq!(row.state, SessionState::Working);
    assert_eq!(row.parent, None);
}

#[test]
fn box_session_row_carries_the_box_name() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "acme/1");
    let row = store
        .session_row(first_session(&store, id))
        .unwrap()
        .unwrap();
    assert_eq!(row.task, Some(BoxName::parse_allow_bare("acme/1").unwrap()));
    assert_eq!(row.ordinal, 1);
    assert_eq!(row.workdir, None);
}

#[test]
fn unknown_session_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    assert_eq!(store.session_row(999).unwrap(), None);
}

#[test]
fn children_list_box_and_host_sessions_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let parent = store.open_host_session("/w", None, None).unwrap();
    let id = new_task(&store, "acme/1");
    let other = new_task(&store, "acme/2");
    end_current(&store, id);
    let box_child = store.open_child_session(id, Some(parent)).unwrap();
    let host_child = store.open_host_session("/w2", None, None).unwrap();
    store
        .writer()
        .unwrap()
        .execute(
            "UPDATE sessions SET parent_session = ?1 WHERE id = ?2",
            (parent, host_child),
        )
        .unwrap();
    let children = store.session_children(parent).unwrap();
    let ids: Vec<i64> = children.iter().map(|c| c.id).collect();
    assert_eq!(ids, [box_child.id, host_child]);
    assert_eq!(
        children[0].task,
        Some(BoxName::parse_allow_bare("acme/1").unwrap())
    );
    assert_eq!(children[0].state, SessionState::Starting);
    assert_eq!(children[0].parent, Some(parent));
    assert_eq!(children[1].task, None);
    assert!(store
        .session_children(first_session(&store, other))
        .unwrap()
        .is_empty());
}
