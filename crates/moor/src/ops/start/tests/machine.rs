//! A start whose repo asks for a machine holds one of the requester's machines, or starts
//! without one and says why; closing the Box frees the machine for the next start.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::config::UserMachine;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::user::Identity;
use moor_core::view::StartView;
use moor_core::BoxId;

use super::{repo_req, script_base_yaml, world, World};
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

fn start(w: &World, name: &str) -> StartView {
    run(&w.cx, &repo_req(name)).unwrap()
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
fn a_matching_machine_is_held_with_no_note() {
    let w = with_machines(&[("mini", "macos")]);
    let v = start(&w, "a");
    assert_eq!(held(&w, &v).as_deref(), Some("mini"));
    let store = w.cx.store().unwrap();
    let got = store.box_machine(BoxId::new(v.boxed.id)).unwrap().unwrap();
    assert_eq!((got.slot, got.ports), (0, vec![5432, 8080]));
    assert!(
        machine_events(&w, &v).is_empty(),
        "start only holds the machine; machine_attached waits for open forwards"
    );
    assert!(notes(&v).is_empty(), "{:?}", v.warnings_inline);
}

#[test]
fn no_matching_machine_starts_without_one_and_says_so() {
    let w = with_machines(&[("pc", "linux")]);
    let v = start(&w, "a");
    assert!(v.boxed.id > 0, "the start is not refused");
    assert_eq!(held(&w, &v), None);
    assert_eq!(
        notes(&v),
        ["machine left out: you have no machine with labels macos"]
    );
    let events = machine_events(&w, &v);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "machine_left_out");
}

#[test]
fn a_second_box_gets_the_next_free_machine() {
    let w = with_machines(&[("mini", "macos"), ("studio", "macos")]);
    let a = start(&w, "a");
    let b = start(&w, "b");
    assert_eq!(held(&w, &a).as_deref(), Some("mini"));
    assert_eq!(held(&w, &b).as_deref(), Some("studio"));
}

#[test]
fn a_held_machine_is_named_with_its_holder_and_closing_frees_it() {
    let w = with_machines(&[("mini", "macos")]);
    let a = start(&w, "a");
    let b = start(&w, "b");
    assert_eq!(held(&w, &b), None);
    assert_eq!(
        notes(&b),
        [format!(
            "machine left out: machine mini is attached to Box {}",
            a.boxed.name
        )
        .as_str()]
    );
    w.cx.store()
        .unwrap()
        .apply(
            Decision::new(BoxId::new(a.boxed.id))
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End)
                .box_input(BoxInput::Released),
        )
        .unwrap();
    let c = start(&w, "c");
    assert_eq!(held(&w, &c).as_deref(), Some("mini"));
}

#[test]
fn another_users_machine_is_never_picked() {
    let w = with_machines(&[]);
    let store = w.cx.store().unwrap();
    let (other, _) = store.enroll(&Identity::local(2000)).unwrap();
    let m = UserMachine::parse("theirs", "dev@mac.example", &["macos".to_owned()]).unwrap();
    store.set_user_machine(other, &m).unwrap();
    let v = start(&w, "a");
    assert_eq!(held(&w, &v), None);
    assert_eq!(notes(&v).len(), 1, "{:?}", v.warnings_inline);
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
fn a_shared_machine_gives_each_box_its_own_slot_until_full() {
    let w = shared(2);
    let a = start(&w, "a");
    let b = start(&w, "b");
    assert_eq!(slot(&w, &a), Some((0, vec![5432, 8080])));
    assert_eq!(slot(&w, &b), Some((1, vec![5442, 8090])));
    let c = start(&w, "c");
    assert_eq!(slot(&w, &c), None);
    assert_eq!(
        notes(&c),
        [format!(
            "machine left out: machine mini is attached to Boxes {}, {}",
            a.boxed.name, b.boxed.name
        )
        .as_str()]
    );
    close(&w, &a);
    let d = start(&w, "d");
    assert_eq!(slot(&w, &d), Some((0, vec![5432, 8080])));
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
    };
    std::thread::scope(|s| {
        for id in &ids {
            let (store, want) = (store.clone(), &want);
            s.spawn(move || store.claim_machine(*id, REQUESTER_USER, want).unwrap());
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
