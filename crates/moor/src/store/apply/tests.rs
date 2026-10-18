//! Tests for the writer: name collisions on insert.

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::domain::task::TaskInput;
use moor_core::BoxName;

use crate::store::tests::open;
use crate::store::StoreError;

/// A new-task decision.
fn insert(name: &str) -> Decision {
    Decision::insert(NewTask {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "r".into(),
    })
}

#[test]
fn open_name_collision_is_name_taken() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    store.apply(insert("dup")).unwrap();
    let err = store.apply(insert("dup")).unwrap_err();
    assert!(matches!(err, StoreError::NameTaken(_)), "{err:?}");
}

#[test]
fn closed_name_can_be_reused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = store.apply(insert("dup")).unwrap().task;
    store
        .apply(Decision::new(id).task_input(TaskInput::Close))
        .unwrap();
    store.apply(insert("dup")).unwrap();
}
