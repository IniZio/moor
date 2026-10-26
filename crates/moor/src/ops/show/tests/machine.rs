//! `show` of one Box names the user machine it holds, its slot and its ports.

use std::sync::Arc;

use moor_core::config::{RepoMachine, UserMachine};
use moor_core::user::Identity;
use moor_core::view::{HeldMachineView, MachineHoldState};

use super::super::run;
use super::{add, cx, req};
use crate::testing::driver::FakeDriver;

#[test]
fn detail_shows_the_held_machine_and_a_box_without_one_shows_none() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let (first, second) = (add(&cx, "a"), add(&cx, "b"));
    add(&cx, "c");
    let store = cx.store().unwrap();
    let (user, _) = store.enroll(&Identity::local(2000)).unwrap();
    let mini = UserMachine::parse("mini", "dev@mini", &["macos".to_owned()])
        .and_then(|m| m.with_capacity(2))
        .unwrap();
    store.set_user_machine(user, &mini).unwrap();
    let want = RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081, 8085],
        ..RepoMachine::default()
    };
    store.claim_machine(first, user, &want, &[]).unwrap();
    store.claim_machine(second, user, &want, &[]).unwrap();
    let held = |name: &str| {
        run(&cx, &req(Some(name), None))
            .unwrap()
            .boxed
            .unwrap()
            .held_machine
    };
    let slot_one = HeldMachineView {
        name: "mini".to_owned(),
        slot: 1,
        ports: vec![8091, 8095],
        state: MachineHoldState::Held,
    };
    assert_eq!(held("b"), Some(slot_one));
    assert_eq!(held("c"), None);
}

#[test]
fn detail_says_an_asleep_claim_waits_while_the_box_is_awake() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let first = add(&cx, "a");
    let store = cx.store().unwrap();
    let (user, _) = store.enroll(&Identity::local(2000)).unwrap();
    let mini = UserMachine::parse("mini", "dev@mini", &["macos".to_owned()]).unwrap();
    store.set_user_machine(user, &mini).unwrap();
    let want = RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
        ..RepoMachine::default()
    };
    store.claim_machine(first, user, &want, &[]).unwrap();
    store.sleep_machine(first).unwrap();

    let view = run(&cx, &req(Some("a"), None)).unwrap();
    let held = view.boxed.unwrap().held_machine.unwrap();
    assert_eq!(held.name, "mini");
    assert_eq!(held.state, MachineHoldState::Waiting);
}
