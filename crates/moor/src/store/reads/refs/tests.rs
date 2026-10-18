//! Tests for Box refs.

use crate::store::tests::{new_task, open};

#[test]
fn add_ref_is_new_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "acme/1");
    assert!(store.add_box_ref(id, "pr", "acme/app#7").unwrap());
    assert!(!store.add_box_ref(id, "pr", "acme/app#7").unwrap());
}

#[test]
fn refs_listed_by_kind_in_insert_order() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "acme/1");
    store.add_box_ref(id, "pr", "acme/app#9").unwrap();
    store.add_box_ref(id, "branch", "feat/x").unwrap();
    store.add_box_ref(id, "pr", "acme/app#3").unwrap();
    assert_eq!(
        store.box_refs(id, "pr").unwrap(),
        ["acme/app#9", "acme/app#3"]
    );
    assert_eq!(store.box_refs(id, "branch").unwrap(), ["feat/x"]);
    assert!(store.box_refs(id, "proof").unwrap().is_empty());
}

#[test]
fn boxes_found_by_kind_and_value() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let first = new_task(&store, "acme/1");
    let second = new_task(&store, "acme/2");
    let other = new_task(&store, "acme/3");
    store.add_box_ref(second, "pr", "acme/app#7").unwrap();
    store.add_box_ref(first, "pr", "acme/app#7").unwrap();
    store.add_box_ref(other, "branch", "acme/app#7").unwrap();
    assert_eq!(
        store.boxes_by_ref("pr", "acme/app#7").unwrap(),
        [first, second]
    );
    assert_eq!(
        store.boxes_by_ref("branch", "acme/app#7").unwrap(),
        [other]
    );
    assert!(store.boxes_by_ref("pr", "acme/app#8").unwrap().is_empty());
}

#[test]
fn unknown_kind_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "acme/1");
    assert!(store.add_box_ref(id, "nope", "x").is_err());
}
