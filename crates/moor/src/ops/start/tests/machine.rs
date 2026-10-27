//! A start holds no machine: the Box claims one of its requester's machines through
//! `machine-hold`, with the ports and syncs its repo's `.moor.yaml` names; closing the Box
//! frees the machine for the next claim.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::config::UserMachine;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::view::StartView;
use moor_core::{BoxId, HoldOp, HoldValue, MachineHoldReq};

use super::{repo_req, script_base_yaml, world, World};
use crate::ops::machine_hold;
use crate::ops::start::run;
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;

const YAML: &str = "machine:\n  labels: [macos]\n  ports: [5432, 8080]\n";

fn with_machines(names: &[(&str, &str)]) -> World {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, Some(YAML));
    let store = w.cx.store().unwrap();
    for (name, label) in names {
        let m = UserMachine::parse(name, "dev@mac.example", &[(*label).to_owned()]).unwrap();
        store.set_user_machine(REQUESTER_USER, &m).unwrap();
    }
    w
}

/// Starts Box `name` and claims a machine for it as its runner would.
fn start(w: &World, name: &str) -> StartView {
    let v = started(w, name);
    claim(w, BoxId::new(v.boxed.id));
    v
}

fn started(w: &World, name: &str) -> StartView {
    run(&w.cx, &repo_req(name)).unwrap()
}

fn claim(w: &World, id: BoxId) -> HoldValue {
    let req = MachineHoldReq {
        id,
        op: HoldOp::repo(),
    };
    machine_hold::run(&w.cx, &req).unwrap()
}

#[test]
fn a_box_whose_repo_has_a_machine_holds_none_right_after_start() {
    let w = with_machines(&[("mini", "macos")]);
    let v = started(&w, "a");
    assert_eq!(held(&w, &v), None);
    assert!(machine_events(&w, &v).is_empty());
    assert!(notes(&v).is_empty(), "{:?}", v.warnings_inline);
}

/// The machine the Box of `v` holds, by name.
fn held(w: &World, v: &StartView) -> Option<String> {
    let store = w.cx.store().unwrap();
    let held = store.box_machine(BoxId::new(v.boxed.id)).unwrap();
    held.map(|h| h.machine.name)
}

/// The Box's machine events as (kind, payload).
fn machine_events(w: &World, v: &StartView) -> Vec<(String, serde_json::Value)> {
    let store = w.cx.store().unwrap();
    let events = store.events(BoxId::new(v.boxed.id), 100).unwrap();
    events
        .into_iter()
        .filter(|e| e.kind.starts_with("machine_"))
        .map(|e| (e.kind, e.payload))
        .collect()
}

fn notes(v: &StartView) -> Vec<&str> {
    v.warnings_inline
        .iter()
        .map(String::as_str)
        .filter(|n| n.starts_with("machine"))
        .collect()
}

#[test]
fn a_second_box_gets_the_next_free_machine() {
    let w = with_machines(&[("mini", "macos"), ("studio", "macos")]);
    let a = start(&w, "a");
    let b = start(&w, "b");
    assert_eq!(held(&w, &a).as_deref(), Some("mini"));
    assert_eq!(held(&w, &b).as_deref(), Some("studio"));
}

fn shared(capacity: u16) -> World {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, Some(YAML));
    let m = UserMachine::parse("mini", "dev@mac.example", &["macos".to_owned()])
        .and_then(|m| m.with_capacity(capacity))
        .unwrap();
    w.cx.store()
        .unwrap()
        .set_user_machine(REQUESTER_USER, &m)
        .unwrap();
    w
}

/// The slot and ports the Box of `v` holds.
fn slot(w: &World, v: &StartView) -> Option<(u16, Vec<u16>)> {
    let store = w.cx.store().unwrap();
    let held = store.box_machine(BoxId::new(v.boxed.id)).unwrap();
    held.map(|h| (h.slot, h.ports))
}

fn close(w: &World, v: &StartView) {
    w.cx.store()
        .unwrap()
        .apply(
            Decision::new(BoxId::new(v.boxed.id))
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End)
                .box_input(BoxInput::Released),
        )
        .unwrap();
}

#[test]
fn adding_a_machine_again_changes_its_capacity() {
    let w = shared(1);
    let m = UserMachine::parse("mini", "dev@mac.example", &["macos".to_owned()])
        .and_then(|m| m.with_capacity(2))
        .unwrap();
    w.cx.store()
        .unwrap()
        .set_user_machine(REQUESTER_USER, &m)
        .unwrap();
    let first = start(&w, "a");
    let second = start(&w, "b");
    assert_eq!(slot(&w, &first).map(|s| s.0), Some(0));
    assert_eq!(slot(&w, &second).map(|s| s.0), Some(1));
}

#[test]
fn closing_a_box_with_no_supervisor_frees_its_machine() {
    let w = shared(1);
    let held_box = start(&w, "a");
    close(&w, &held_box);
    assert_eq!(slot(&w, &held_box), None);
    let kinds: Vec<String> = machine_events(&w, &held_box)
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(kinds, ["machine_detached"]);
}

#[test]
fn parallel_claims_never_share_a_slot() {
    let w = shared(4);
    let views: Vec<StartView> = ["a", "b", "c", "d"].iter().map(|n| start(&w, n)).collect();
    let store = w.cx.store().unwrap();
    let ids: Vec<BoxId> = views.iter().map(|v| BoxId::new(v.boxed.id)).collect();
    for id in &ids {
        store.detach_machine(*id).unwrap();
    }
    let want = moor_core::config::RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8080],
        ..moor_core::config::RepoMachine::default()
    };
    std::thread::scope(|s| {
        for id in &ids {
            let (store, want) = (store.clone(), &want);
            s.spawn(move || {
                let op = HoldOp::repo();
                store
                    .hold_machine(*id, REQUESTER_USER, &op, Some(want), &[])
                    .unwrap()
            });
        }
    });
    let mut slots: Vec<u16> = views
        .iter()
        .filter_map(|v| slot(&w, v))
        .map(|s| s.0)
        .collect();
    slots.sort_unstable();
    assert_eq!(slots, [0, 1, 2, 3]);
}

#[test]
fn the_hold_keeps_what_its_syncs_leave_alone() {
    let w = with_machines(&[("mini", "macos")]);
    let yaml =
        "machine:\n  labels: [macos]\n  sync: {presets: [node, rust], exclude: [app/build]}\n";
    script_base_yaml(&w, Some(yaml));
    let v = start(&w, "a");
    let store = w.cx.store().unwrap();
    let got = store.box_machine(BoxId::new(v.boxed.id)).unwrap().unwrap();
    let want = [
        "Pods",
        ".gradle",
        "DerivedData",
        "*.xcworkspace/xcuserdata",
        ".dart_tool",
        "app/build",
    ];
    assert_eq!(got.sync_exclude, want);
}
