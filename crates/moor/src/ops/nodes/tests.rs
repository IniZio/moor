//! Tests for `node-list`.

use std::collections::BTreeMap;
use std::sync::Arc;

use moor_core::config::{EnrolledMachine, NodeConfig, UserConfig};
use moor_core::request::MachineLink;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId, Viewer};
use moor_core::view::node::NodeScope;

use super::list;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn cx(dir: &std::path::Path) -> Cx {
    let config = UserConfig {
        nodes: BTreeMap::from([(
            "fast".to_owned(),
            NodeConfig {
                runtime: "static".to_owned(),
                ..NodeConfig::default()
            },
        )]),
        ..UserConfig::default()
    };
    Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_config(config)
}

/// Enrolls `who` with a pool machine and returns the user and a link for the machine.
fn enroll(cx: &Cx, who: &str, machine: &str, online: bool) -> (UserId, MachineLink) {
    let store = cx.store().unwrap();
    let (user, _) = store.enroll(&Identity::parse(who).unwrap(), None).unwrap();
    let mut m = EnrolledMachine::parse(machine, KEY, &[], 1).unwrap();
    m.pool = Some("cloudhypervisor".to_owned());
    let machine_id = store.enroll_machine(user, &m).unwrap();
    let link = MachineLink {
        machine_id,
        online,
        since: Ts::from_millis(7_000),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: Vec::new(),
    };
    (user, link)
}

/// A context with alice's online `mini` and bob's offline `rack`.
fn two_users(dir: &std::path::Path) -> (Cx, UserId, UserId) {
    let cx = cx(dir);
    let (alice, a) = enroll(&cx, "authgear:a", "mini", true);
    let (bob, b) = enroll(&cx, "authgear:b", "rack", false);
    cx.store().unwrap().set_machine_links(&[a, b]).unwrap();
    (cx, alice, bob)
}

fn names(cx: &Cx) -> Vec<(String, NodeScope, bool)> {
    list(cx)
        .unwrap()
        .nodes
        .into_iter()
        .map(|n| (n.name, n.scope, n.online))
        .collect()
}

#[test]
fn a_user_sees_shared_nodes_and_its_own_but_not_another_users() {
    let dir = tempfile::tempdir().unwrap();
    let (base, alice, bob) = two_users(dir.path());

    let as_alice = base.with_viewer(Viewer::User(alice));
    assert_eq!(
        names(&as_alice),
        vec![
            ("fast".to_owned(), NodeScope::Shared, true),
            ("mini".to_owned(), NodeScope::Personal, true),
        ]
    );
    let listed = list(&as_alice).unwrap().nodes;
    assert_eq!(
        listed.iter().map(|n| n.owner).collect::<Vec<_>>(),
        [None, Some(alice)]
    );

    let seen = list(&as_alice.with_viewer(Viewer::User(bob)))
        .unwrap()
        .nodes;
    let seen: Vec<_> = seen
        .iter()
        .map(|n| (n.name.as_str(), n.online, n.offline_since))
        .collect();
    assert_eq!(seen, [("fast", true, None), ("rack", false, Some(7))]);
}

#[test]
fn the_operator_sees_every_node() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _) = two_users(dir.path());
    let all: Vec<String> = names(&cx).into_iter().map(|n| n.0).collect();
    assert_eq!(all, ["fast", "mini", "rack"]);
}

#[test]
fn with_no_shared_node_a_user_sees_only_its_own_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let base = cx(dir.path()).with_config(UserConfig::default());
    assert!(list(&base).unwrap().nodes.is_empty());
    let (alice, _) = enroll(&base, "authgear:a", "mini", true);
    let listed = list(&base.with_viewer(Viewer::User(alice))).unwrap().nodes;
    let seen: Vec<_> = listed.iter().map(|n| (n.name.as_str(), n.scope)).collect();
    assert_eq!(seen, [("mini", NodeScope::Personal)]);
}
