//! A start naming no node, on a controller whose only node is a shared machine an operator
//! enrolled with `moor node add`.

use moor_core::config::EnrolledMachine;
use moor_core::request::MachineLink;
use moor_core::seam::clock::Ts;
use moor_core::user::Viewer;
use moor_core::view::node::PickOutcome;
use moor_core::{BoxId, ErrorKind};

use super::{req, world_without_nodes, World};
use crate::cx::Cx;
use crate::ops::{nodes, start};
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

/// A world whose only node is the shared machine `local-agent`, its link up if `online`.
fn shared_only(online: bool) -> World {
    let mut w = world_without_nodes(FakeDriver::new());
    let cx = std::mem::replace(
        &mut w.cx,
        Cx::fake_without_nodes(w.dir.path(), w.driver.clone(), w.proc.clone()),
    );
    w.cx = cx.with_viewer(Viewer::User(REQUESTER_USER));
    let machine = EnrolledMachine::parse("local-agent", KEY, &[], 2)
        .and_then(|m| m.with_pool("boxes"))
        .unwrap();
    let machine_id =
        w.cx.store()
            .unwrap()
            .enroll_shared_machine(&machine)
            .unwrap();
    let link = MachineLink {
        provides: None,
        machine_id,
        online,
        since: Ts::from_millis(1),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: Vec::new(),
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        memory: None,
    };
    nodes::set_links(&w.cx, &crate::testing::one_runner(vec![link])).unwrap();
    w
}

#[test]
fn a_start_naming_no_node_lands_on_the_only_shared_node_as_node_list_says() {
    let w = shared_only(true);
    let pick = nodes::list(&w.cx).unwrap().start.unwrap();
    assert_eq!(
        (pick.node.as_deref(), pick.outcome),
        (Some("local-agent"), PickOutcome::Starts)
    );

    let v = start::run(&w.cx, &req("demo")).unwrap();
    assert!(!v.queued);
    let spec = w.cx.store().unwrap().box_spec(BoxId::new(v.boxed.id));
    assert_eq!(
        spec.unwrap().node.map(|n| n.to_string()).as_deref(),
        Some("local-agent")
    );
}

#[test]
fn a_start_naming_no_node_refuses_when_the_only_shared_node_is_offline() {
    let w = shared_only(false);
    let e = start::run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert_eq!(
        e.error.to_string(),
        "local-agent is offline for 0m: wait for it to come back"
    );
    assert_eq!(w.driver.calls(), Vec::<String>::new());
}
