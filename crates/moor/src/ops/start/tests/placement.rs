//! Where a start lands: a shared node, or the requester's first personal node by default; a full
//! node keeps the Box Pending on it until a Box there closes or its link comes back.

use moor_core::config::{parse_user, EnrolledMachine};
use moor_core::request::MachineLink;
use moor_core::seam::clock::Ts;
use moor_core::view::StartView;
use moor_core::{BoxId, BoxName, CloseReq, ErrorKind};

use super::{req, world_with, World};
use crate::cx::Cx;
use crate::ops::{close, nodes, start};
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

/// Shared nodes `ci` (the default) and `big`, one Box each, both on the fake runtime.
fn world() -> World {
    let yaml = "nodes: {ci: {runtime: local-container, max_boxes: 1}, \
                big: {runtime: local-container, max_boxes: 1}}\nbox: {node: ci}";
    world_with(FakeDriver::new(), parse_user("config.yaml", yaml).unwrap())
}

/// Enrolls the requester's machine `name` offering `cloudhypervisor`, linked when `online` is set.
fn enrolled(w: &World, name: &str, online: Option<bool>, max_boxes: Option<u32>) -> i64 {
    let store = w.cx.store().unwrap();
    let m = EnrolledMachine::parse(name, KEY, &[], 2)
        .unwrap()
        .with_pool("cloudhypervisor")
        .unwrap();
    let machine_id = store.enroll_machine(REQUESTER_USER, &m).unwrap();
    if let Some(online) = online {
        link(w, machine_id, online, max_boxes);
    }
    machine_id
}

fn link(w: &World, machine_id: i64, online: bool, max_boxes: Option<u32>) {
    let link = MachineLink {
        machine_id,
        online,
        since: Ts::from_millis(1),
        version: None,
        protocol: None,
        max_boxes,
        cache_gb: None,
        features: Vec::new(),
    };
    nodes::set_links(&w.cx, &[link]).unwrap();
}

fn held(v: &StartView) -> &str {
    v.boxed.driver.as_ref().map_or("", |d| d.provider.as_str())
}

fn node_of(w: &World, v: &StartView) -> Option<String> {
    let spec = w.cx.store().unwrap().box_spec(BoxId::new(v.boxed.id));
    spec.unwrap().node.map(|n| n.to_string())
}

#[test]
fn a_start_that_fits_a_shared_node_is_bound_there() {
    let w = world();
    let v = start::run(&w.cx, &req("demo")).unwrap();
    assert!(!v.queued);
    assert_eq!(held(&v), "local-container");
    assert_eq!(node_of(&w, &v).as_deref(), Some("ci"));
}

#[test]
fn the_requesters_first_enrolled_node_is_the_default_and_the_start_says_so() {
    let w = world();
    enrolled(&w, "hv", Some(true), None);
    let v = start::run(&w.cx, &req("demo")).unwrap();
    assert_eq!(held(&v), "cloudhypervisor@hv");
    assert_eq!(node_of(&w, &v).as_deref(), Some("hv"));
    assert!(
        v.warnings_inline
            .iter()
            .any(|m| m.contains("starting on your node hv")),
        "{:?}",
        v.warnings_inline
    );
}

#[test]
fn an_offline_or_never_linked_default_node_refuses_the_start_and_names_the_skip() {
    for online in [Some(false), None] {
        let w = world();
        enrolled(&w, "hv", online, None);
        let e = start::run(&w.cx, &req("demo")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Refused, "{e:?}");
        let msg = e.error.to_string();
        assert!(
            msg.contains("hv is offline") && msg.contains("--node ci"),
            "{msg}"
        );
        assert_eq!(w.driver.calls(), Vec::<String>::new(), "no driver call");
        assert!(w.cx.store().unwrap().open_boxes().unwrap().is_empty());
    }
}

#[test]
fn a_start_on_a_full_node_is_pending_there_with_the_reason() {
    let w = world();
    start::run(&w.cx, &req("holder")).unwrap();
    let v = start::run(&w.cx, &req("waiter")).unwrap();
    assert!(v.queued);
    assert_eq!(node_of(&w, &v).as_deref(), Some("ci"));
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
    let reason = row.unwrap().unwrap().state_reason.unwrap_or_default();
    assert!(reason.contains("ci is full (1 of 1 Boxes)"), "{reason}");
    assert_eq!(w.driver.calls(), ["acquire"]);
}

#[test]
fn a_close_starts_the_oldest_pending_box_of_its_node_only() {
    let w = world();
    start::run(&w.cx, &req("holder")).unwrap();
    let older = start::run(&w.cx, &req("older")).unwrap();
    let younger = start::run(&w.cx, &req("younger")).unwrap();
    let other = BoxId::new(start::run(&w.cx, &req("other")).unwrap().boxed.id);
    w.cx.store()
        .unwrap()
        .set_box_spec(
            other,
            &moor_core::decide::schedule::BoxSpec {
                labels: Vec::new(),
                node: moor_core::name::NodeName::parse("big").ok(),
            },
        )
        .unwrap();
    assert!(older.queued && younger.queued);
    let closed = close::run(
        &w.cx,
        &CloseReq {
            target: BoxName::parse_allow_bare("holder").unwrap().into(),
            force: true,
        },
    )
    .unwrap();
    assert_eq!(closed.promoted, ["older"]);
}

#[test]
fn a_link_coming_up_starts_the_boxes_pending_on_its_node() {
    let w = world();
    let hv = enrolled(&w, "hv", Some(true), Some(1));
    let holder = start::run(&w.cx, &req("holder")).unwrap();
    let waiter = start::run(&w.cx, &req("waiter")).unwrap();
    assert!(waiter.queued);
    assert_eq!(node_of(&w, &waiter).as_deref(), Some("hv"));
    link(&w, hv, false, Some(1));
    let store = w.cx.store().unwrap();
    store
        .apply(
            moor_core::domain::decision::Decision::new(BoxId::new(holder.boxed.id))
                .box_input(moor_core::domain::boxes::BoxInput::DriverMissing),
        )
        .unwrap();
    assert!(store
        .box_row(BoxId::new(waiter.boxed.id))
        .unwrap()
        .is_some_and(|r| r.boxed.is_none()));
    link(&w, hv, true, Some(1));
    let row = store.box_row(BoxId::new(waiter.boxed.id)).unwrap().unwrap();
    assert_eq!(
        row.boxed.map(|b| b.provider.as_str().to_owned()).as_deref(),
        Some("cloudhypervisor@hv")
    );
}

#[test]
fn two_starts_racing_for_one_free_slot_bind_one_and_leave_one_pending() {
    let w = world();
    let (dir, config) = (w.dir.path().to_owned(), w.cx.config().clone());
    let (driver, surface, proc, forge) = (&w.driver, &w.surface, &w.proc, &w.forge);
    let starter = |name: &'static str| {
        let (dir, config) = (dir.clone(), config.clone());
        let (driver, surface, proc, forge) =
            (driver.clone(), surface.clone(), proc.clone(), forge.clone());
        move || {
            let cx = Cx::fake_with(&dir, driver, surface, proc, config).with_forge(forge);
            start::run(&cx, &req(name)).unwrap()
        }
    };
    let (a, b) = std::thread::scope(|s| {
        let (a, b) = (s.spawn(starter("one")), s.spawn(starter("two")));
        (a.join().unwrap(), b.join().unwrap())
    });
    let mut got = [a.queued, b.queued];
    got.sort_unstable();
    assert_eq!(got, [false, true]);
    assert_eq!(w.driver.calls(), ["acquire"]);
}
