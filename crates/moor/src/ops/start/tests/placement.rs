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
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
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
                mem_mib: 0,
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

const VOLUMES: &str = "cache: {volumes: [{name: sdk, path: /opt/sdk}]}\n";

fn planned_volumes(w: &World) -> Option<usize> {
    w.forge.answer(
        super::repo(),
        Ok(super::repo_at(super::BASE, Some(VOLUMES))),
    );
    start::run(&w.cx, &super::repo_req("demo")).unwrap();
    w.driver.acquired_plans().first().map(|p| p.volumes().len())
}

fn cached_world(features: &[&str], online: bool) -> World {
    let yaml = "nodes: {ci: {runtime: local-container}}\nbox: {node: ci}";
    let driver = FakeDriver::new().cache_volumes();
    let w = world_with(driver, parse_user("config.yaml", yaml).unwrap());
    let id = enrolled(&w, "hv", None, None);
    let link = MachineLink {
        machine_id: id,
        online,
        since: Ts::from_millis(1),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: features.iter().map(|f| (*f).to_owned()).collect(),
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
    };
    nodes::set_links(&w.cx, &[link]).unwrap();
    w
}

#[test]
fn a_personal_node_that_reports_cache_volumes_gets_them() {
    let w = cached_world(&["cache-volume"], true);
    assert!(planned_volumes(&w) > Some(0));
}

#[test]
fn a_personal_node_that_does_not_report_cache_volumes_starts_cold() {
    for features in [&["other"][..], &[]] {
        let w = cached_world(features, true);
        assert_eq!(planned_volumes(&w), Some(0), "{features:?}");
    }
}

#[test]
fn a_shared_node_keeps_the_runtimes_own_cache_volumes() {
    let w = world_with(
        FakeDriver::new().cache_volumes(),
        parse_user(
            "config.yaml",
            "nodes: {ci: {runtime: local-container}}\nbox: {node: ci}",
        )
        .unwrap(),
    );
    assert!(planned_volumes(&w) > Some(0));
}

/// Links machine `machine_id` online with `mem_mib` of host memory and a `reserve_mib` reserve.
fn link_memory(w: &World, machine_id: i64, mem_mib: u64, reserve_mib: u64) {
    let link = MachineLink {
        machine_id,
        online: true,
        since: Ts::from_millis(1),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: Vec::new(),
        mem_mib: Some(mem_mib),
        cpus: None,
        reserve_mib: Some(reserve_mib),
    };
    nodes::set_links(&w.cx, &[link]).unwrap();
}

/// A node with room for exactly one small Box (4 GiB) after its reserve.
fn one_small_box_world() -> World {
    let w = world();
    let hv = enrolled(&w, "hv", Some(true), None);
    link_memory(&w, hv, 8192, 4096);
    w
}

#[test]
fn an_asleep_box_still_counts_against_its_nodes_memory() {
    let w = one_small_box_world();
    let holder = start::run(&w.cx, &req("holder")).unwrap();
    assert!(!holder.queued);
    w.cx.store()
        .unwrap()
        .apply(
            moor_core::domain::decision::Decision::new(BoxId::new(holder.boxed.id))
                .box_input(moor_core::domain::boxes::BoxInput::Sleep),
        )
        .unwrap();
    let waiter = start::run(&w.cx, &req("waiter")).unwrap();
    assert!(waiter.queued);
    let row = w.cx.store().unwrap().box_row(BoxId::new(waiter.boxed.id));
    let reason = row.unwrap().unwrap().state_reason.unwrap_or_default();
    assert!(reason.contains("hv has 0 GiB free of 4 GiB"), "{reason}");
}

#[test]
fn a_close_that_frees_memory_starts_the_pending_box() {
    let w = one_small_box_world();
    start::run(&w.cx, &req("holder")).unwrap();
    let waiter = start::run(&w.cx, &req("waiter")).unwrap();
    assert!(waiter.queued);
    let closed = close::run(
        &w.cx,
        &CloseReq {
            target: BoxName::parse_allow_bare("holder").unwrap().into(),
            force: true,
        },
    )
    .unwrap();
    assert_eq!(closed.promoted, ["waiter"]);
}

/// A node whose pause frees memory, with room for `boxes` small Boxes after its reserve.
fn suspending_world(boxes: u64) -> World {
    let w = world();
    let hv = enrolled(&w, "hv", Some(true), None);
    let link = MachineLink {
        machine_id: hv,
        online: true,
        since: Ts::from_millis(1),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: vec!["pause-frees-memory".to_owned()],
        mem_mib: Some(4096 + boxes * 4096),
        cpus: None,
        reserve_mib: Some(4096),
    };
    nodes::set_links(&w.cx, &[link]).unwrap();
    w
}

fn started(w: &World, name: &str) -> BoxId {
    let v = start::run(&w.cx, &req(name)).unwrap();
    assert!(!v.queued, "{name} should fit");
    BoxId::new(v.boxed.id)
}

fn apply(w: &World, d: moor_core::domain::decision::Decision) {
    w.cx.store().unwrap().apply(d).unwrap();
}

/// Settles `id`'s agent Idle, then lets a minute pass so later Boxes are idle for less.
fn idle(w: &World, id: BoxId) {
    use moor_core::domain::session_state::SessionInput;
    apply(
        w,
        moor_core::domain::decision::Decision::new(id).session(SessionInput::AgentQuiet),
    );
    w.cx.clock().sleep(std::time::Duration::from_secs(60));
}

/// Makes the agents of `ids` report Idle, so a start's settle pass keeps idle Boxes idle.
fn agents_report_idle(mut w: World, ids: &[BoxId]) -> World {
    use moor_core::activity::{HookKind, HookRead, HookRecord, PaneRead, Probes};
    let idle = Probes {
        hook: HookRead::Fresh(HookRecord {
            kind: HookKind::Idle,
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: w.cx.clock().now(),
            turn: None,
        }),
        pane: PaneRead::Absent,
    };
    let engine = ids
        .iter()
        .fold(crate::testing::engine::FakeEngine::new(), |e, id| {
            e.probes(&format!("moor-{}", id.get()), idle.clone())
        });
    w.cx = w.cx.with_engine(std::sync::Arc::new(engine));
    w
}

fn put_to_sleep(w: &World, id: BoxId) {
    use moor_core::domain::box_state::BoxLifecycleInput;
    apply(
        w,
        moor_core::domain::decision::Decision::new(id)
            .lifecycle(BoxLifecycleInput::Sleep {
                auto: false,
                reason: None,
            })
            .box_input(moor_core::domain::boxes::BoxInput::Sleep),
    );
}

fn lifecycle(w: &World, id: BoxId) -> moor_core::domain::box_state::BoxLifecycle {
    w.cx.store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .lifecycle
}

fn asleep_reason(w: &World, id: BoxId) -> Option<String> {
    let ev =
        w.cx.store()
            .unwrap()
            .newest_event_of(id, "asleep")
            .unwrap()?;
    ev.payload
        .get("reason")
        .and_then(|r| r.as_str())
        .map(str::to_owned)
}

#[test]
fn an_asleep_box_on_a_node_whose_pause_frees_memory_holds_none() {
    let w = suspending_world(1);
    let holder = started(&w, "holder");
    put_to_sleep(&w, holder);
    started(&w, "waiter");
}

#[test]
fn a_start_on_a_full_node_suspends_its_longest_idle_box_and_says_why() {
    use moor_core::domain::box_state::BoxLifecycle;
    let w = suspending_world(2);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, a);
    idle(&w, b);
    let w = agents_report_idle(w, &[a, b]);
    started(&w, "c");
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Asleep);
    assert_eq!(lifecycle(&w, b), BoxLifecycle::Ready);
    assert_eq!(asleep_reason(&w, a).as_deref(), Some("made room for c"));
    let row = w.cx.store().unwrap().box_row(a).unwrap().unwrap();
    assert_eq!(row.state_reason.as_deref(), Some("made room for c"));
}

#[test]
fn a_working_box_is_never_suspended_and_the_start_waits() {
    use moor_core::domain::box_state::BoxLifecycle;
    let w = suspending_world(1);
    let busy = started(&w, "busy");
    let waiter = start::run(&w.cx, &req("waiter")).unwrap();
    assert!(waiter.queued);
    assert_eq!(lifecycle(&w, busy), BoxLifecycle::Ready);
}

#[test]
fn suspending_every_idle_box_still_short_suspends_none() {
    use moor_core::domain::box_state::BoxLifecycle;
    let w = suspending_world(2);
    let (a, busy) = (started(&w, "a"), started(&w, "busy"));
    idle(&w, a);
    let w = agents_report_idle(w, &[a]);
    let store = w.cx.store().unwrap();
    let mut spec = store.box_spec(busy).unwrap();
    spec.mem_mib = 6144;
    store.set_box_spec(busy, &spec).unwrap();
    let waiter = start::run(&w.cx, &req("waiter")).unwrap();
    assert!(waiter.queued);
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Ready);
    let row = store.box_row(BoxId::new(waiter.boxed.id)).unwrap().unwrap();
    let reason = row.state_reason.unwrap_or_default();
    assert!(reason.contains("hv has 0 GiB free of 8 GiB"), "{reason}");
}

#[test]
fn waking_a_suspended_box_suspends_an_idle_one_to_make_room() {
    use moor_core::domain::box_state::BoxLifecycle;
    let w = suspending_world(1);
    let a = started(&w, "a");
    put_to_sleep(&w, a);
    let b = started(&w, "b");
    idle(&w, b);
    crate::ops::wake::run(
        &w.cx,
        &moor_core::BoxReq {
            box_name: w.cx.store().unwrap().box_row(a).unwrap().unwrap().name,
        },
    )
    .unwrap();
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Ready);
    assert_eq!(lifecycle(&w, b), BoxLifecycle::Asleep);
    assert_eq!(asleep_reason(&w, b).as_deref(), Some("made room for a"));
}

#[test]
fn a_wake_with_no_idle_box_to_suspend_is_refused_and_stays_asleep() {
    use moor_core::domain::box_state::BoxLifecycle;
    let w = suspending_world(1);
    let a = started(&w, "a");
    put_to_sleep(&w, a);
    started(&w, "busy");
    let err = crate::ops::wake::run(
        &w.cx,
        &moor_core::BoxReq {
            box_name: w.cx.store().unwrap().box_row(a).unwrap().unwrap().name,
        },
    )
    .unwrap_err();
    assert!(
        err.error.to_string().contains("hv has 0 GiB free"),
        "{}",
        err.error
    );
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Asleep);
}
