//! A start names the node it wants, and its repo can ask for node labels.

use moor_core::config::{parse_user, EnrolledMachine};
use moor_core::{BoxId, ErrorKind, StartReq};

use super::{repo_req, req, run, script_base_yaml, world_with, World};
use crate::testing::driver::FakeDriver;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

/// Shared nodes `ci` (the default) and `big` with `gpu`, one Box each.
fn world() -> World {
    let yaml = "nodes: {ci: {runtime: local-container, max_boxes: 1}, \
                big: {runtime: local-container, labels: [gpu], max_boxes: 1}}\nbox: {node: ci}";
    world_with(FakeDriver::new(), parse_user("config.yaml", yaml).unwrap())
}

fn on(name: &str, node: &str) -> StartReq {
    StartReq {
        node: Some(node.to_owned()),
        ..req(name)
    }
}

fn node_of(w: &World, id: i64) -> Option<String> {
    let spec = w.cx.store().unwrap().box_spec(BoxId::new(id)).unwrap();
    spec.node.map(|n| n.to_string())
}

#[test]
fn a_start_naming_a_shared_node_runs_there() {
    let w = world();
    let v = run(&w.cx, &on("demo", "big")).unwrap();
    assert!(!v.queued);
    assert_eq!(node_of(&w, v.boxed.id).as_deref(), Some("big"));
}

#[test]
fn a_start_naming_another_users_node_finds_no_such_node() {
    let w = world();
    let store = w.cx.store().unwrap();
    let id = moor_core::user::Identity::parse("authgear:other").unwrap();
    let (other, _) = store.enroll(&id, None).unwrap();
    let mut m = EnrolledMachine::parse("theirs", KEY, &[], 1).unwrap();
    m.pool = Some("cloudhypervisor".to_owned());
    store.enroll_machine(other, &m).unwrap();
    let e = run(&w.cx, &on("demo", "theirs")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused, "{e:?}");
    assert!(
        e.error.to_string().contains("no node named theirs"),
        "{e:?}"
    );
}

#[test]
fn a_start_naming_a_full_node_is_pending_on_it() {
    let w = world();
    run(&w.cx, &on("holder", "big")).unwrap();
    let v = run(&w.cx, &on("waiter", "big")).unwrap();
    assert!(v.queued);
    assert_eq!(node_of(&w, v.boxed.id).as_deref(), Some("big"));
}

#[test]
fn repo_labels_the_default_node_has_start_there() {
    let yaml = "nodes: {ci: {runtime: local-container}, \
                big: {runtime: local-container, labels: [gpu]}}\nbox: {node: big}";
    let w = world_with(FakeDriver::new(), parse_user("config.yaml", yaml).unwrap());
    script_base_yaml(&w, Some("node: {labels: [gpu]}\n"));
    let v = run(&w.cx, &repo_req("demo")).unwrap();
    assert_eq!(node_of(&w, v.boxed.id).as_deref(), Some("big"));
}

#[test]
fn repo_labels_no_node_has_refuse_the_start() {
    let w = world();
    script_base_yaml(&w, Some("node: {labels: [arm]}\n"));
    let e = run(&w.cx, &repo_req("demo")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused, "{e:?}");
    assert!(
        e.error.to_string().contains("no node has labels arm"),
        "{e:?}"
    );
}

#[test]
fn a_repo_that_names_a_node_is_refused() {
    let w = world();
    script_base_yaml(&w, Some("node: {name: big}\n"));
    let e = run(&w.cx, &repo_req("demo")).unwrap_err();
    assert!(e.error.to_string().contains("unknown field"), "{e:?}");
}
