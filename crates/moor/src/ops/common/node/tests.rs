//! Boxes that predate node placement are put on the node they ran on.

use std::sync::Arc;

use moor_core::config::{parse_user, EnrolledMachine};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};

use super::{backfill, configured};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;
use moor_core::user::UserId;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

struct World {
    cx: Cx,
    _dir: tempfile::TempDir,
}

fn world(yaml: &str) -> World {
    let dir = tempfile::tempdir().unwrap();
    let cx = Cx::fake_with(
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        parse_user("config.yaml", yaml).unwrap(),
    );
    World { cx, _dir: dir }
}

/// A Box held on `provider` with `handle`, as one made before nodes existed.
fn legacy(w: &World, name: &str, provider: &str, handle: &str) -> BoxId {
    let store = w.cx.store().unwrap();
    let new = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(new)).unwrap().box_id;
    let requested = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new(provider)),
            ..Fields::default()
        });
    store.apply(requested).unwrap();
    let acquired = BoxInput::Acquired {
        handle: BoxHandle::new(handle),
    };
    store.apply(Decision::new(id).box_input(acquired)).unwrap();
    id
}

fn node_of(w: &World, id: BoxId) -> Option<String> {
    let spec = w.cx.store().unwrap().box_spec(id).unwrap();
    spec.node.map(|n| n.to_string())
}

fn enroll(w: &World, name: &str) -> i64 {
    let m = EnrolledMachine::parse(name, KEY, &[], 1)
        .unwrap()
        .with_pool("cloudhypervisor")
        .unwrap();
    w.cx.store()
        .unwrap()
        .enroll_machine(UserId::new(1), &m)
        .unwrap()
}

fn run(w: &World) {
    backfill(&w.cx, w.cx.store().unwrap()).unwrap();
}

const TWO: &str = "nodes: {ci: {runtime: local-container}, big: {runtime: local-container}, \
                   vm: {runtime: other}}\nbox: {node: big}";

#[test]
fn a_host_provider_goes_to_that_machines_node() {
    let w = world(TWO);
    enroll(&w, "mini");
    let id = legacy(&w, "a", "cloudhypervisor@mini", "h");
    run(&w);
    assert_eq!(node_of(&w, id).as_deref(), Some("mini"));
}

#[test]
fn a_pool_handle_goes_to_the_machine_it_names() {
    let w = world(TWO);
    enroll(&w, "other");
    let machine = enroll(&w, "mini");
    let handle = format!("cloudhypervisor/{machine}/moor-ctl-1");
    let id = legacy(&w, "a", "cloudhypervisor", &handle);
    run(&w);
    assert_eq!(node_of(&w, id).as_deref(), Some("mini"));
}

#[test]
fn a_plain_provider_goes_to_the_shared_node_with_that_runtime() {
    let w = world(TWO);
    let id = legacy(&w, "a", "other", "h");
    run(&w);
    assert_eq!(node_of(&w, id).as_deref(), Some("vm"));
}

#[test]
fn a_plain_provider_shared_by_several_nodes_goes_to_the_default() {
    let w = world(TWO);
    let id = legacy(&w, "a", "local-container", "h");
    run(&w);
    assert_eq!(node_of(&w, id).as_deref(), Some("big"));
}

#[test]
fn a_box_matching_no_node_stays_unplaced_with_a_warning() {
    let w = world(TWO);
    let id = legacy(&w, "a", "gone", "h");
    run(&w);
    assert_eq!(node_of(&w, id), None);
    let warned = w.cx.take_warnings();
    assert_eq!(warned.len(), 1);
    assert!(warned
        .iter()
        .all(|w| w.message.contains(&format!("Box {}", id.get()))));
}

#[test]
fn backfilling_twice_changes_nothing_more() {
    let w = world(TWO);
    let id = legacy(&w, "a", "other", "h");
    run(&w);
    run(&w);
    assert_eq!(node_of(&w, id).as_deref(), Some("vm"));
    assert!(w.cx.take_warnings().is_empty());
}

#[test]
fn a_backfilled_box_counts_toward_its_nodes_capacity() {
    let w = world(TWO);
    legacy(&w, "a", "other", "h");
    let (shared, _) = configured(w.cx.config()).unwrap();
    let used = |w: &World| {
        let all = w.cx.store().unwrap().nodes(&shared).unwrap();
        all.iter().find(|n| n.name.as_str() == "vm").unwrap().used
    };
    assert_eq!(used(&w), 0);
    run(&w);
    assert_eq!(used(&w), 1);
}
