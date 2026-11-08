//! Tests for `node-list`.

use std::collections::BTreeMap;
use std::sync::Arc;

use moor_core::config::{EnrolledMachine, NodeConfig, UserConfig};
use moor_core::name::BoxId;
use moor_core::request::{BoxHostMemory, MachineLink};
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId, Viewer};
use moor_core::view::node::{NodeScope, PickOutcome};

use super::list;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

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
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        warm_starts: None,
        memory: None,
    };
    (user, link)
}

/// A context with alice's online `mini` and bob's offline `rack`.
fn two_users(dir: &std::path::Path) -> (Cx, UserId, UserId) {
    let cx = cx(dir);
    let (alice, a) = enroll(&cx, "authgear:a", "mini", true);
    let (bob, b) = enroll(&cx, "authgear:b", "rack", false);
    cx.store()
        .unwrap()
        .set_machine_links(&crate::testing::runner(), false, &[a, b], cx.clock().now())
        .unwrap();
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

fn held(cx: &Cx, node: &str) -> Option<u64> {
    list(cx)
        .unwrap()
        .nodes
        .into_iter()
        .find(|n| n.name == node)
        .unwrap()
        .box_memory_mib
}

fn on(cx: &Cx, machine_id: i64, box_id: i64, host_memory_mib: u64) -> BoxHostMemory {
    BoxHostMemory {
        machine_id,
        controller: cx.store().unwrap().controller_id().unwrap(),
        box_id: BoxId::new(box_id),
        host_memory_mib,
        resume_memory_mib: None,
    }
}

#[test]
fn a_node_shows_the_sum_its_boxes_hold_and_a_later_report_without_one_drops_it() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _) = two_users(dir.path());
    assert_eq!(held(&cx, "mini"), None);
    let id = cx
        .store()
        .unwrap()
        .machine_keys()
        .unwrap()
        .first()
        .unwrap()
        .machine_id;
    let store = cx.store().unwrap();
    store
        .set_box_memory(&[on(&cx, id, 1, 2048), on(&cx, id, 2, 0), on(&cx, id, 3, 512)])
        .unwrap();
    assert_eq!(held(&cx, "mini"), Some(2560));
    assert_eq!(held(&cx, "fast"), None);
    store.set_box_memory(&[on(&cx, id, 2, 0)]).unwrap();
    assert_eq!(held(&cx, "mini"), Some(0));
    store.set_box_memory(&[]).unwrap();
    assert_eq!(held(&cx, "mini"), None);
}

#[test]
fn memory_for_a_machine_that_is_not_enrolled_is_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _) = two_users(dir.path());
    cx.store()
        .unwrap()
        .set_box_memory(&[on(&cx, 999, 1, 100)])
        .unwrap();
    assert!(cx.store().unwrap().box_memory_totals().unwrap().is_empty());
}

#[test]
fn memory_for_a_box_another_controller_named_is_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _) = two_users(dir.path());
    let store = cx.store().unwrap();
    let id = store.machine_keys().unwrap().first().unwrap().machine_id;
    let mut other = on(&cx, id, 1, 4096);
    other.controller = moor_core::box_key::ControllerId::parse("ffffffff").unwrap();
    assert_ne!(other.controller, store.controller_id().unwrap());
    store.set_box_memory(&[other, on(&cx, id, 2, 512)]).unwrap();
    assert_eq!(held(&cx, "mini"), Some(512));
}

#[test]
fn a_start_picks_the_users_online_personal_node() {
    let dir = tempfile::tempdir().unwrap();
    let (base, alice, _) = two_users(dir.path());
    let pick = list(&base.with_viewer(Viewer::User(alice)))
        .unwrap()
        .start
        .unwrap();
    assert_eq!(pick.node.as_deref(), Some("mini"));
    assert_eq!(pick.outcome, PickOutcome::Starts);
}

#[test]
fn a_start_for_a_user_whose_personal_node_is_offline_is_refused_not_sent_to_a_shared_one() {
    let dir = tempfile::tempdir().unwrap();
    let (base, _, bob) = two_users(dir.path());
    let pick = list(&base.with_viewer(Viewer::User(bob)))
        .unwrap()
        .start
        .unwrap();
    assert_eq!(pick.outcome, PickOutcome::Refused);
    assert!(pick.reason.unwrap().contains("rack is offline"));
}

#[test]
fn the_operator_gets_no_start_pick() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _) = two_users(dir.path());
    assert!(list(&cx).unwrap().start.is_none());
}
