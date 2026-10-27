#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::config::{RepoMachine, UserMachine};
use moor_core::seam::proc::Tool;
use moor_core::user::Identity;
use moor_core::HoldOp;

use super::{start, Stop, World};
use crate::proxy::front::Hold;
use crate::proxy::supervisor::machine::hold_of;
use crate::store::TunnelWant;
use crate::testing::proc::ChildReply;

/// A Box whose requester holds the machine `mini`, forwarding Box ports 8081 and 3001.
fn held_machine() -> World {
    let w = World::new([]);
    let (user, _) = w
        .store
        .enroll(&Identity::parse("slack:T1:U1").unwrap())
        .unwrap();
    let mini = UserMachine::parse("mini", "dev@mini", &["macos".to_owned()]).unwrap();
    w.store.set_user_machine(user, &mini).unwrap();
    let want = RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081, 3001],
        ..RepoMachine::default()
    };
    w.store
        .hold_machine(w.id, user, &HoldOp::repo(), Some(&want), &[])
        .unwrap();
    w.proc.on_child(Tool::Ssh, &[], ChildReply::Hang);
    w
}

fn machine_kinds(w: &World) -> Vec<String> {
    let events = w.store.events(w.id, 50).unwrap();
    let kinds = events.into_iter().map(|e| e.kind);
    kinds.filter(|k| k.starts_with("machine_")).collect()
}

#[test]
fn the_first_tick_opens_the_forwards_and_reports_the_machine_attached() {
    let w = held_machine();
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    assert!(run.tick().is_none());

    assert_eq!(machine_kinds(&w), ["machine_attached"]);
    let ssh = w.proc.calls().into_iter().filter(|c| c.tool == Tool::Ssh);
    assert_eq!(ssh.count(), 1, "one ssh child serves every port");
    assert!(run.tick().is_none());
    assert_eq!(
        machine_kinds(&w),
        ["machine_attached"],
        "a live link is reported once"
    );
}

#[test]
fn closing_the_box_ends_the_forwards_and_frees_the_machine() {
    let w = held_machine();
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());

    w.close_box();
    let stop = run.tick();
    assert!(matches!(stop, Some(Stop::Closed)));
    assert_eq!(run.finish(Stop::Closed), 0);

    assert!(
        w.store.box_machine(w.id).unwrap().is_none(),
        "the machine is free"
    );
    assert_eq!(machine_kinds(&w), ["machine_attached", "machine_detached"]);
    assert!(!w.proc.dropped().is_empty(), "the ssh child was ended");
}

#[test]
fn slot_one_forwards_each_port_moved_by_ten_at_the_same_number_on_both_ends() {
    let w = World::new([]);
    let (user, _) = w
        .store
        .enroll(&Identity::parse("slack:T1:U1").unwrap())
        .unwrap();
    let mini = UserMachine::parse("mini", "dev@mini", &["macos".to_owned()])
        .and_then(|m| m.with_capacity(2))
        .unwrap();
    w.store.set_user_machine(user, &mini).unwrap();
    let want = RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
        ..RepoMachine::default()
    };
    let other = super::working_box(&w.store, &w.driver, "other");
    w.store
        .hold_machine(other, user, &HoldOp::repo(), Some(&want), &[])
        .unwrap();
    w.store
        .hold_machine(w.id, user, &HoldOp::repo(), Some(&want), &[])
        .unwrap();
    w.proc.on_child(Tool::Ssh, &[], ChildReply::Hang);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    let calls = w.proc.calls();
    let ssh = calls.iter().find(|c| c.tool == Tool::Ssh).unwrap();
    let forwards: Vec<&str> = ssh
        .args
        .iter()
        .filter_map(|a| a.split_once(":127.0.0.1:").map(|(p, _)| p))
        .collect();
    assert_eq!(forwards, ["8091"], "{:?}", ssh.args);
}

/// `held_machine` with `mini` taking `capacity` Boxes.
fn mini_of(capacity: u16) -> (World, moor_core::user::UserId) {
    let w = World::new([]);
    let (user, _) = w
        .store
        .enroll(&Identity::parse("slack:T1:U1").unwrap())
        .unwrap();
    let mini = UserMachine::parse("mini", "dev@mini", &["macos".to_owned()])
        .and_then(|m| m.with_capacity(capacity))
        .unwrap();
    w.store.set_user_machine(user, &mini).unwrap();
    w.store
        .hold_machine(w.id, user, &HoldOp::repo(), Some(&want_8081()), &[])
        .unwrap();
    w.proc.on_child(Tool::Ssh, &[], ChildReply::Hang);
    (w, user)
}

fn want_8081() -> RepoMachine {
    RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
        ..RepoMachine::default()
    }
}

fn machine_payloads(w: &World) -> Vec<serde_json::Value> {
    let events = w.store.events(w.id, 50).unwrap();
    let machine = events
        .into_iter()
        .filter(|e| e.kind.starts_with("machine_"));
    machine.map(|e| e.payload).collect()
}

fn last_forwards(w: &World) -> Vec<String> {
    let calls = w.proc.calls();
    let ssh = calls.iter().rev().find(|c| c.tool == Tool::Ssh).unwrap();
    let forwards = ssh.args.iter().filter_map(|a| a.split_once(":127.0.0.1:"));
    forwards.map(|(p, _)| p.to_owned()).collect()
}

#[test]
fn a_parked_tick_frees_the_slot_but_keeps_the_claim() {
    let (w, _) = mini_of(1);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());

    w.store.request_tunnel(w.id, TunnelWant::Parked).unwrap();
    assert!(run.tick().is_none());

    assert_eq!(machine_kinds(&w), ["machine_attached", "machine_detached"]);
    assert_eq!(machine_payloads(&w)[1]["asleep"], true);
    assert!(w.store.box_machine(w.id).unwrap().is_none());
    let claim = w.store.claimed_machine(w.id).unwrap().unwrap();
    assert_eq!(
        claim.state,
        moor_core::view::MachineHoldState::Asleep,
        "the Box keeps its claim on mini"
    );
}

#[test]
fn an_unparked_tick_takes_a_free_slot_and_forwards_its_ports() {
    let (w, user) = mini_of(2);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    w.store.request_tunnel(w.id, TunnelWant::Parked).unwrap();
    assert!(run.tick().is_none());
    let other = super::working_box(&w.store, &w.driver, "other");
    w.store
        .hold_machine(other, user, &HoldOp::repo(), Some(&want_8081()), &[])
        .unwrap();

    w.store.request_tunnel(w.id, TunnelWant::Up).unwrap();
    assert!(run.tick().is_none());

    let kinds = machine_kinds(&w);
    assert_eq!(kinds.last().map(String::as_str), Some("machine_attached"));
    let attached = machine_payloads(&w).pop().unwrap();
    assert_eq!(attached["ports"], serde_json::json!([8091]));
    assert_eq!(last_forwards(&w), ["8091"]);
    assert_eq!(w.store.box_machine(w.id).unwrap().unwrap().slot, 1);
}

#[test]
fn a_full_machine_reports_waiting_once_and_wakes_when_a_slot_frees() {
    let (w, user) = mini_of(1);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    w.store.request_tunnel(w.id, TunnelWant::Parked).unwrap();
    assert!(run.tick().is_none());
    let other = super::working_box(&w.store, &w.driver, "other");
    w.store
        .hold_machine(other, user, &HoldOp::repo(), Some(&want_8081()), &[])
        .unwrap();

    w.store.request_tunnel(w.id, TunnelWant::Up).unwrap();
    assert!((0..3).all(|_| run.tick().is_none()));

    let waiting = ["machine_attached", "machine_detached", "machine_waiting"];
    assert_eq!(machine_kinds(&w), waiting);
    let reason = machine_payloads(&w)[2]["reason"].to_string();
    assert!(reason.contains("Box other"), "{reason}");
    assert!(w.store.box_machine(w.id).unwrap().is_none());

    w.store.detach_machine(other).unwrap();
    assert!(run.tick().is_none());
    assert_eq!(machine_kinds(&w).last().unwrap(), "machine_attached");
    assert_eq!(last_forwards(&w), ["8081"]);
}

fn missing_reason(w: &World) -> String {
    match hold_of(&w.store, w.id) {
        Hold::Missing { reason, .. } => reason,
        Hold::Held(_) => String::new(),
    }
}

#[test]
fn a_box_without_a_claim_is_told_its_repo_asks_for_none() {
    let w = World::new([]);
    assert!(missing_reason(&w).starts_with("this Box has no machine: "));
}

#[test]
fn an_asleep_claim_is_released_while_parked_and_waiting_once_asked_up() {
    let w = held_machine();
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    assert!(matches!(hold_of(&w.store, w.id), Hold::Held(h) if h.name == "mini"));

    w.store.request_tunnel(w.id, TunnelWant::Parked).unwrap();
    assert!(run.tick().is_none());
    assert_eq!(
        missing_reason(&w),
        "machine mini is released while this Box sleeps"
    );

    w.store.request_tunnel(w.id, TunnelWant::Up).unwrap();
    assert_eq!(
        missing_reason(&w),
        "machine mini has no free slot yet; this Box takes one as soon as another Box frees it"
    );
}
