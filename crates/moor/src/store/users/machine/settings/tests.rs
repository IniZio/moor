//! A node's settings: written only at the revision the writer saw, and read by the scheduler.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use moor_core::config::{EnrolledMachine, UserMachine, BOXES_HOST};
use moor_core::request::CapEdit;
use moor_core::user::{Identity, UserId};

use super::{Change, NodeSettings, Wrote};
use crate::store::tests::open;
use crate::store::Store;

fn cap(n: u32) -> CapEdit {
    CapEdit::Set(NonZeroU32::new(n).unwrap())
}

fn rig() -> (Store, UserId, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (user, _) = store
        .enroll(&Identity::parse("herdr:T1:U1").unwrap(), None)
        .unwrap();
    let host = UserMachine::parse("host", &["linux".to_owned()])
        .unwrap()
        .with_pool(BOXES_HOST)
        .unwrap();
    store.set_user_machine(user, &host).unwrap();
    (store, user, dir)
}

fn mine(store: &Store, user: UserId) -> NodeSettings {
    store.node_settings(Some(user), "host").unwrap().unwrap()
}

fn written(w: Wrote) -> NodeSettings {
    match w {
        Wrote::Written(row) => Some(row),
        Wrote::Stale { .. } => None,
    }
    .expect("the write was not refused")
}

fn change(max_boxes: CapEdit, labels: Option<&[String]>) -> Change<'_> {
    Change { max_boxes, labels }
}

#[test]
fn a_new_node_has_no_cap_and_no_write() {
    let (store, user, _dir) = rig();
    let node = mine(&store, user);
    assert_eq!(
        (node.max_boxes, node.revision, node.edited_by, node.labels),
        (None, 0, None, vec!["linux".to_owned()])
    );
    assert!(store.node_settings(None, "host").unwrap().is_none());
}

#[test]
fn a_write_sets_the_cap_and_each_later_one_adds_a_revision() {
    let (store, user, _dir) = rig();
    let node = mine(&store, user);
    let first = written(
        store
            .set_node_settings(&node, "host", &change(cap(2), None), "me", Some(0))
            .unwrap(),
    );
    assert_eq!((first.max_boxes, first.revision), (Some(2), 1));
    assert_eq!(first.edited_by.as_deref(), Some("me"));
    assert_eq!(first.labels, ["linux"], "the labels were left alone");
    let labels = vec!["gpu".to_owned()];
    let second = written(
        store
            .set_node_settings(
                &node,
                "host",
                &change(CapEdit::Keep, Some(&labels)),
                "me",
                Some(1),
            )
            .unwrap(),
    );
    assert_eq!(
        (second.max_boxes, second.revision, second.labels),
        (Some(2), 2, labels)
    );
    let cleared = written(
        store
            .set_node_settings(&node, "host", &change(CapEdit::Clear, None), "me", None)
            .unwrap(),
    );
    assert_eq!((cleared.max_boxes, cleared.revision), (None, 3));
}

#[test]
fn a_write_at_a_revision_the_node_has_left_is_refused_and_changes_nothing() {
    let (store, user, _dir) = rig();
    let node = mine(&store, user);
    store
        .set_node_settings(&node, "host", &change(cap(2), None), "me", Some(0))
        .unwrap();
    let stale = store
        .set_node_settings(&node, "host", &change(cap(9), None), "you", Some(0))
        .unwrap();
    assert_eq!(stale, Wrote::Stale { current: Some(1) });
    let kept = mine(&store, user);
    assert_eq!(
        (kept.max_boxes, kept.edited_by.as_deref()),
        (Some(2), Some("me"))
    );
}

#[test]
fn the_scheduler_reads_the_stored_cap_and_labels_and_an_agent_offer_is_ignored() {
    let (store, user, _dir) = rig();
    let node = |store: &Store| {
        let nodes = store.nodes(&BTreeMap::new()).unwrap();
        nodes
            .into_iter()
            .find(|n| n.name.as_str() == "host")
            .unwrap()
    };
    assert_eq!(node(&store).max_boxes, None);
    let labels = vec!["gpu".to_owned()];
    store
        .set_node_settings(
            &mine(&store, user),
            "host",
            &change(cap(3), Some(&labels)),
            "me",
            None,
        )
        .unwrap();
    let read = node(&store);
    assert_eq!((read.max_boxes, read.labels), (Some(3), labels));
}

#[test]
fn a_shared_machine_is_found_with_no_owner_and_pairing_again_keeps_the_cap() {
    let (store, user, _dir) = rig();
    let shared = EnrolledMachine::parse("shared", &key(), &[], 1)
        .and_then(|m| m.with_pool(BOXES_HOST))
        .unwrap();
    store.enroll_shared_machine(&shared).unwrap();
    let node = store.node_settings(None, "shared").unwrap().unwrap();
    assert_eq!(node.owner, None);
    store
        .set_node_settings(&node, "shared", &change(cap(1), None), "operator", None)
        .unwrap();
    store.enroll_shared_machine(&shared).unwrap();
    assert_eq!(
        store
            .node_settings(None, "shared")
            .unwrap()
            .unwrap()
            .max_boxes,
        Some(1)
    );
    assert_eq!(
        mine(&store, user).max_boxes,
        None,
        "another node's cap is its own"
    );
    assert_eq!(store.node_revisions().unwrap().len(), 1);
}

fn key() -> String {
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl".to_owned()
}
