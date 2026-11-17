//! Tests for `node-settings-get` and `node-settings-set`.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use moor_core::config::{EnrolledMachine, NodeConfig, UserConfig, BOXES_HOST};
use moor_core::request::{CapEdit, NodeSettingsGetReq, NodeSettingsSetReq};
use moor_core::user::{Identity, UserId, Viewer};
use moor_core::ErrorKind;

use super::{get, set};
use crate::cx::Cx;
use crate::ops::nodes::list;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn rig(dir: &std::path::Path) -> (Cx, UserId, UserId) {
    let config = UserConfig {
        nodes: BTreeMap::from([(
            "fast".to_owned(),
            NodeConfig {
                runtime: "static".to_owned(),
                max_boxes: NonZeroU32::new(5),
                ..NodeConfig::default()
            },
        )]),
        ..UserConfig::default()
    };
    let cx = Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_config(config);
    let store = cx.store().unwrap();
    let mut users = [UserId::new(0); 2];
    for (slot, who) in users.iter_mut().zip(["authgear:a", "authgear:b"]) {
        let (user, _) = store.enroll(&Identity::parse(who).unwrap(), None).unwrap();
        let mini = EnrolledMachine::parse("mini", KEY, &[], 1)
            .and_then(|m| m.with_pool(BOXES_HOST))
            .unwrap();
        store.enroll_machine(user, &mini).unwrap();
        *slot = user;
    }
    let shared = EnrolledMachine::parse("local", KEY, &[], 1)
        .and_then(|m| m.with_pool(BOXES_HOST))
        .unwrap();
    store.enroll_shared_machine(&shared).unwrap();
    let [alice, bob] = users;
    (cx, alice, bob)
}

fn ask(name: &str, max_boxes: CapEdit, expected: Option<u64>) -> NodeSettingsSetReq {
    NodeSettingsSetReq {
        identity: None,
        name: name.to_owned(),
        max_boxes,
        labels: None,
        expected_revision: expected,
    }
}

fn cap(n: u32) -> CapEdit {
    CapEdit::Set(NonZeroU32::new(n).unwrap())
}

fn read(cx: &Cx, name: &str) -> moor_core::view::node::NodeSettingsValue {
    get(
        cx,
        &NodeSettingsGetReq {
            identity: None,
            name: name.to_owned(),
        },
    )
    .unwrap()
}

#[test]
fn an_owner_sets_the_cap_and_labels_of_their_node_and_node_list_shows_them() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, alice, _) = rig(dir.path());
    let me = cx.with_viewer(Viewer::User(alice));
    assert_eq!(read(&me, "mini").max_boxes, None);
    let written = set(
        &me,
        &NodeSettingsSetReq {
            labels: Some(vec!["gpu".to_owned(), "gpu".to_owned()]),
            ..ask("mini", cap(2), Some(0))
        },
    )
    .unwrap();
    assert_eq!(
        (
            written.max_boxes,
            written.revision,
            written.labels.as_slice()
        ),
        (Some(2), 1, ["gpu".to_owned()].as_slice())
    );
    assert_eq!(
        written.edited_by.as_deref(),
        Some(&*format!("user {}", alice.get()))
    );
    let listed = list(&me).unwrap().nodes;
    let node = listed.iter().find(|n| n.name == "mini").unwrap();
    assert_eq!(
        (
            node.max_boxes,
            node.labels.as_slice(),
            node.revision,
            node.editable
        ),
        (Some(2), ["gpu".to_owned()].as_slice(), 1, true)
    );
    let cleared = set(&me, &ask("mini", CapEdit::Clear, Some(1))).unwrap();
    assert_eq!((cleared.max_boxes, cleared.revision), (None, 2));
}

#[test]
fn a_stale_revision_is_refused_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, alice, _) = rig(dir.path());
    let me = cx.with_viewer(Viewer::User(alice));
    set(&me, &ask("mini", cap(2), Some(0))).unwrap();
    let err = set(&me, &ask("mini", cap(9), Some(0))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("revision 1"),
        "{}",
        err.error
    );
    assert_eq!(read(&me, "mini").max_boxes, Some(2));
}

#[test]
fn a_user_edits_only_their_own_node_and_not_a_shared_or_configured_one() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, alice, _) = rig(dir.path());
    let me = cx.with_viewer(Viewer::User(alice));
    for shared in ["local", "fast"] {
        let seen = read(&me, shared);
        assert!(!seen.can_edit, "{shared}");
        let err = set(&me, &ask(shared, cap(1), None)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refused, "{shared}");
    }
    assert_eq!(
        read(&me, "fast").max_boxes,
        Some(5),
        "a configured node reads as configured"
    );
    let err = get(
        &me,
        &NodeSettingsGetReq {
            identity: None,
            name: "nowhere".to_owned(),
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[test]
fn each_user_edits_the_node_of_that_name_that_is_theirs() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, alice, bob) = rig(dir.path());
    let as_alice = cx.with_viewer(Viewer::User(alice));
    set(&as_alice, &ask("mini", cap(1), None)).unwrap();
    assert_eq!(read(&as_alice, "mini").max_boxes, Some(1));
    let as_bob = as_alice.with_viewer(Viewer::User(bob));
    assert_eq!(read(&as_bob, "mini").max_boxes, None);
}

#[test]
fn the_operator_edits_a_shared_node_and_not_a_users() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _) = rig(dir.path());
    let op = cx.with_viewer(Viewer::Operator);
    let written = set(&op, &ask("local", cap(3), None)).unwrap();
    assert_eq!(
        (written.max_boxes, written.edited_by.as_deref()),
        (Some(3), Some("operator"))
    );
    let err = set(&op, &ask("mini", cap(3), None)).unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::Refused,
        "the operator has no personal node named mini"
    );
}

#[test]
fn a_label_that_is_not_accepted_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, alice, _) = rig(dir.path());
    let err = set(
        &cx.with_viewer(Viewer::User(alice)),
        &NodeSettingsSetReq {
            labels: Some(vec!["Not A Label".to_owned()]),
            ..ask("mini", CapEdit::Keep, None)
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}
