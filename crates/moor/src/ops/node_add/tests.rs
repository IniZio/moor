//! Tests for `node-add`.

use std::collections::BTreeMap;
use std::sync::Arc;

use moor_core::config::{EnrolledMachine, NodeConfig, UserConfig};
use moor_core::request::{MachineLink, NodeAddReq};
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, Viewer};
use moor_core::view::node::NodeScope;
use moor_core::ErrorKind;

use super::run;
use crate::cx::Cx;
use crate::ops::nodes::list;
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

fn req(name: &str) -> NodeAddReq {
    NodeAddReq {
        name: name.to_owned(),
        pubkey: KEY.to_owned(),
        labels: vec!["linux".to_owned()],
    }
}

#[test]
fn an_added_node_is_shared_offline_until_its_agent_links_and_online_after() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());

    let added = run(&cx, &req("local")).unwrap();

    let node = |cx: &Cx| {
        list(cx)
            .unwrap()
            .nodes
            .into_iter()
            .find(|n| n.name == "local")
            .unwrap()
    };
    let before = node(&cx);
    assert_eq!(
        (before.scope, before.owner, before.online),
        (NodeScope::Shared, None, false)
    );
    assert_eq!(before.labels, ["linux"]);
    let link = MachineLink {
        provides: None,
        machine_id: added.machine_id,
        online: true,
        since: Ts::from_millis(7_000),
        version: None,
        protocol: None,
        max_boxes: Some(4),
        cache_gb: None,
        features: Vec::new(),
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        memory: None,
    };
    cx.store()
        .unwrap()
        .set_machine_links(&crate::testing::runner(), false, &[link], cx.clock().now())
        .unwrap();
    let after = node(&cx);
    assert_eq!(
        (after.online, after.max_boxes),
        (true, None),
        "the cap an agent offers is not the node's cap"
    );
}

#[test]
fn every_user_sees_an_added_node_as_shared() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    run(&cx, &req("local")).unwrap();
    let store = cx.store().unwrap();
    let (alice, _) = store
        .enroll(&Identity::parse("authgear:a").unwrap(), None)
        .unwrap();

    let seen = list(&cx.with_viewer(Viewer::User(alice))).unwrap().nodes;

    let shared: Vec<_> = seen
        .iter()
        .filter(|n| n.scope == NodeScope::Shared)
        .map(|n| n.name.as_str())
        .collect();
    assert_eq!(shared, ["fast", "local"]);
}

#[test]
fn adding_the_same_name_again_keeps_the_machine() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let first = run(&cx, &req("local")).unwrap();
    assert_eq!(run(&cx, &req("local")).unwrap(), first);
}

#[test]
fn a_name_a_config_node_or_a_users_machine_holds_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let store = cx.store().unwrap();
    let (alice, _) = store
        .enroll(&Identity::parse("authgear:a").unwrap(), None)
        .unwrap();
    let mini = EnrolledMachine::parse("mini", KEY, &[], 1).unwrap();
    store.enroll_machine(alice, &mini).unwrap();

    for taken in ["fast", "mini"] {
        let err = run(&cx, &req(taken)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refused, "{taken}");
    }
}

#[test]
fn a_bad_name_or_key_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let bad_key = NodeAddReq {
        pubkey: "not a key".to_owned(),
        ..req("local")
    };
    for bad in [req("Local"), req(""), bad_key] {
        let err = run(&cx, &bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
    }
}
