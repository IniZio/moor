//! Tests for reading Sessions by id and by parent.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::domain::session_state::SessionState;
use moor_core::{BoxId, BoxName};

use crate::store::tests::{new_box, open};
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
fn host_session_row_has_no_box_and_keeps_its_fields() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = store
        .open_host_session("/w", Some("a"), Some("s1"))
        .unwrap();
    let row = store.session_row(id).unwrap().unwrap();
    assert_eq!(row.box_name, None);
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
    let id = new_box(&store, "acme/1");
    let row = store
        .session_row(first_session(&store, id))
        .unwrap()
        .unwrap();
    assert_eq!(
        row.box_name,
        Some(BoxName::parse_allow_bare("acme/1").unwrap())
    );
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
    let id = new_box(&store, "acme/1");
    let other = new_box(&store, "acme/2");
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
        children[0].box_name,
        Some(BoxName::parse_allow_bare("acme/1").unwrap())
    );
    assert_eq!(children[0].state, SessionState::Starting);
    assert_eq!(children[0].parent, Some(parent));
    assert_eq!(children[1].box_name, None);
    assert!(store
        .session_children(first_session(&store, other))
        .unwrap()
        .is_empty());
}

#[test]
fn live_sessions_skip_ended_ones_and_come_back_in_id_order() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let a = store.open_host_session("/a", None, None).unwrap();
    let gone = store.open_host_session("/gone", None, None).unwrap();
    let b = store.open_host_session("/b", None, None).unwrap();
    store
        .writer()
        .unwrap()
        .execute("UPDATE sessions SET state = 'ended' WHERE id = ?", [gone])
        .unwrap();
    let ids: Vec<i64> = store
        .live_sessions()
        .unwrap()
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, vec![a, b]);
}

#[test]
fn live_sessions_carry_the_box_name_and_parent_of_a_box_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let root = store.open_host_session("/w", None, None).unwrap();
    let id = new_box(&store, "acme/1");
    end_current(&store, id);
    let child = store.open_child_session(id, Some(root)).unwrap();
    let name = BoxName::parse_allow_bare("acme/1").unwrap();
    let shape = |rows: Vec<super::SessionRecord>| {
        rows.into_iter()
            .map(|r| (r.id, r.box_name, r.parent))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shape(store.live_sessions().unwrap()),
        [
            (root, None, None),
            (child.id, Some(name.clone()), Some(root))
        ]
    );
    assert_eq!(
        shape(store.session_children(root).unwrap()),
        [(child.id, Some(name), Some(root))]
    );
}
