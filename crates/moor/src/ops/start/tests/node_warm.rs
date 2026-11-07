//! Warm starts on a personal node: only an agent that says so gets them, each node's checkpoints
//! stay its own, and the disk floor holds.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::config::EnrolledMachine;
use moor_core::domain::event::ProviderStep;
use moor_core::domain::layers::chain;
use moor_core::name::NodeName;
use moor_core::request::{MachineLink, MemoryReport, WarmStarts};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Checkpoint;
use moor_core::{BoxName, BoxReq, CloseReq, ProviderId, Warning};

use super::warm::{after_a_partial_setup_with, events, start, LINES};
use super::World;
use crate::ops::{close, nodes};
use crate::store::checkpoints::Lineage;
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
const TOTAL_MIB: u64 = 500_000;
const FLOOR_MIB: u64 = 50_000;

/// A link of `machine` whose agent says `warm`, its last report showing `free_mib` of disk.
fn link(machine_id: i64, warm: Option<WarmStarts>, free_mib: u64, at: i64) -> MachineLink {
    MachineLink {
        machine_id,
        online: true,
        since: Ts::from_millis(1),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: Vec::new(),
        mem_mib: Some(64_000),
        cpus: None,
        reserve_mib: None,
        warm_starts: warm,
        memory: Some(MemoryReport {
            at: Ts::from_millis(at),
            available_mib: 60_000,
            total_mib: 64_000,
            psi_centi: None,
            psi_full_centi: None,
            disk_free_mib: Some(free_mib),
            disk_total_mib: Some(TOTAL_MIB),
        }),
    }
}

/// Enrolls the requester's node `hv` and links it with `warm`; its machine id.
fn node(w: &World, warm: Option<WarmStarts>) -> i64 {
    let m = EnrolledMachine::parse("hv", KEY, &[], 8)
        .unwrap()
        .with_pool("external")
        .unwrap();
    let store = w.cx.store().unwrap();
    let machine = store.enroll_machine(REQUESTER_USER, &m).unwrap();
    let at = w.cx.clock().now().as_millis();
    nodes::set_links(
        &w.cx,
        &crate::testing::one_runner(vec![link(machine, warm, 400_000, at)]),
    )
    .unwrap();
    machine
}

fn roomy() -> FakeDriver {
    FakeDriver::new().with_checkpoint_disk(TOTAL_MIB, 400_000, 10)
}

fn warned(w: &World, code: &str) -> Vec<Warning> {
    let all = w.cx.take_warnings();
    all.into_iter().filter(|x| x.code == code).collect()
}

#[test]
fn an_agent_that_serves_checkpoints_gets_a_saved_layer_forked_on_the_next_box() {
    let (w, _) = after_a_partial_setup_with(roomy(), LINES, |w| {
        node(w, Some(WarmStarts::On));
    });
    assert_eq!(w.driver.checkpoint_ids(), ["ckpt-1", "ckpt-2"]);
    let second = start(&w, "second", "abc123", LINES);
    assert!(w.driver.calls().contains(&"fork ckpt-2".to_owned()));
    assert_eq!(events(&w, second, "warm_started").len(), 1);
}

#[test]
fn a_warm_start_records_the_steps_the_fork_reports_and_none_when_it_reports_none() {
    let steps = vec![
        ProviderStep::parse("timing root-flatten ms=7641 bytes=5").unwrap(),
        ProviderStep::parse("timing fork-total ms=24115").unwrap(),
    ];
    let (w, _) = after_a_partial_setup_with(roomy().with_fork_steps(steps), LINES, |w| {
        node(w, Some(WarmStarts::On));
    });
    let second = start(&w, "second", "abc123", LINES);
    let event = &events(&w, second, "warm_started")[0];
    assert_eq!(
        event["steps"],
        serde_json::json!([
            {"name": "root-flatten", "ms": 7641, "detail": "bytes=5"},
            {"name": "fork-total", "ms": 24115},
        ]),
        "{event}"
    );

    let (w, _) = after_a_partial_setup_with(roomy(), LINES, |w| {
        node(w, Some(WarmStarts::On));
    });
    let second = start(&w, "second", "abc123", LINES);
    let event = &events(&w, second, "warm_started")[0];
    assert!(event.get("steps").is_none(), "{event}");
    assert!(event.get("fork_ms").is_some(), "{event}");
}

#[test]
fn a_retry_that_forks_keeps_the_machine_of_the_box() {
    let mut machine = 0;
    let (w, _) = after_a_partial_setup_with(roomy(), LINES, |w| {
        machine = node(w, Some(WarmStarts::On));
    });
    let retry = BoxReq {
        box_name: crate::testing::box_name("first"),
    };
    crate::ops::retry::run(&w.cx, &retry).unwrap();
    assert_eq!(w.driver.fork_seats(), [Some(machine)]);
}

#[test]
fn an_agent_that_says_no_or_never_said_starts_cold_and_saves_nothing() {
    let off = WarmStarts::Off {
        why: "doctor failed".to_owned(),
    };
    for warm in [Some(off), None] {
        let (w, _) = after_a_partial_setup_with(roomy(), LINES, |w| {
            node(w, warm.clone());
        });
        assert!(w.driver.checkpoint_ids().is_empty(), "{warm:?}");
        let second = start(&w, "second", "abc123", LINES);
        assert!(events(&w, second, "warm_started").is_empty(), "{warm:?}");
    }
}

#[test]
fn a_save_below_the_floor_is_skipped_with_its_warning() {
    let low = FakeDriver::new().with_checkpoint_disk(TOTAL_MIB, FLOOR_MIB - 1, 10);
    let (w, _) = after_a_partial_setup_with(low, LINES, |w| {
        node(w, Some(WarmStarts::On));
    });
    assert!(w.driver.checkpoint_ids().is_empty());
    assert!(!warned(&w, "checkpoint_skipped").is_empty());
}

/// Records a checkpoint of another machine under the same provider and name.
fn on_another_machine(w: &World, machine: i64) {
    let store = w.cx.store().unwrap();
    let key = chain(REQUESTER_USER, "o/r", "elsewhere", "", &[]).remove(0);
    let lineage = Lineage {
        user: REQUESTER_USER,
        repo: "o/r".to_owned(),
        chain: key.clone(),
    };
    let made = Checkpoint {
        id: "theirs".to_owned(),
        bytes: None,
        parent: None,
    };
    let home = ProviderId::new(format!("external@hv/{}", machine + 1));
    store
        .record_checkpoint(&home, &key, &lineage, &made)
        .unwrap();
}

fn close(w: &World) {
    let req = CloseReq {
        target: BoxName::parse_allow_bare("first").unwrap().into(),
        force: true,
    };
    close::run(&w.cx, &req).unwrap();
}

fn deleted(w: &World) -> Vec<String> {
    let calls = w.driver.calls();
    let gone = calls
        .iter()
        .filter_map(|c| c.strip_prefix("delete-checkpoint "));
    gone.map(str::to_owned).collect()
}

#[test]
fn a_report_below_the_floor_frees_this_nodes_disk_for_a_box_there_and_no_other_nodes() {
    let mut machine = 0;
    let (w, _) = after_a_partial_setup_with(roomy(), LINES, |w| {
        machine = node(w, Some(WarmStarts::On));
    });
    on_another_machine(&w, machine);
    close(&w);
    let second = start(&w, "second", "def456", LINES);
    let at = w.cx.clock().now().as_millis() + 20_000;
    w.driver.set_free_mib(FLOOR_MIB - 1);
    let report = link(machine, Some(WarmStarts::On), FLOOR_MIB - 1, at);
    nodes::set_links(&w.cx, &crate::testing::one_runner(vec![report])).unwrap();
    let gone = deleted(&w);
    assert!(!gone.is_empty(), "{:?}", w.driver.calls());
    assert!(!gone.contains(&"theirs".to_owned()), "{gone:?}");
    assert!(!events(&w, second, "checkpoint_evicted").is_empty());
}

#[test]
fn a_report_below_the_floor_frees_disk_for_a_box_waiting_for_it_but_not_for_nobody() {
    let mut machine = 0;
    let (w, _) = after_a_partial_setup_with(roomy(), LINES, |w| {
        machine = node(w, Some(WarmStarts::On));
    });
    close(&w);
    w.driver.set_free_mib(FLOOR_MIB - 1);
    let mut at = w.cx.clock().now().as_millis() + 20_000;
    let low = |at| link(machine, Some(WarmStarts::On), FLOOR_MIB - 1, at);
    nodes::set_links(&w.cx, &crate::testing::one_runner(vec![low(at)])).unwrap();
    assert!(deleted(&w).is_empty(), "no Box wants the room");
    let second = start(&w, "second", "def456", LINES);
    assert!(w
        .cx
        .store()
        .unwrap()
        .pending_on(&NodeName::parse("hv").unwrap())
        .unwrap()
        .contains(&second));
    at += 20_000;
    nodes::set_links(&w.cx, &crate::testing::one_runner(vec![low(at)])).unwrap();
    assert!(!deleted(&w).is_empty(), "{:?}", w.driver.calls());
    assert!(!events(&w, second, "checkpoint_evicted").is_empty());
}

#[test]
fn a_report_above_the_floor_frees_nothing() {
    let mut machine = 0;
    let (w, _) = after_a_partial_setup_with(roomy(), LINES, |w| {
        machine = node(w, Some(WarmStarts::On));
    });
    close(&w);
    let at = w.cx.clock().now().as_millis() + 20_000;
    let report = link(machine, Some(WarmStarts::On), FLOOR_MIB + 1, at);
    nodes::set_links(&w.cx, &crate::testing::one_runner(vec![report])).unwrap();
    assert!(deleted(&w).is_empty());
}

#[test]
fn node_ls_says_on_or_off_with_the_reason() {
    let (w, _) = after_a_partial_setup_with(roomy(), LINES, |w| {
        node(w, Some(WarmStarts::On));
    });
    let view = nodes::list(&w.cx).unwrap();
    let hv = view.nodes.iter().find(|n| n.name == "hv").unwrap();
    assert_eq!(hv.warm_starts, Some(WarmStarts::On));
    let off = WarmStarts::Off {
        why: "stock hypervisor".to_owned(),
    };
    let at = w.cx.clock().now().as_millis() + 20_000;
    let report = link(1, Some(off.clone()), 400_000, at);
    nodes::set_links(&w.cx, &crate::testing::one_runner(vec![report])).unwrap();
    let view = nodes::list(&w.cx).unwrap();
    let hv = view.nodes.iter().find(|n| n.name == "hv").unwrap();
    assert_eq!(hv.warm_starts, Some(off));
}
