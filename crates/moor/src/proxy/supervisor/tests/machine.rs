use moor_core::config::{RepoMachine, UserMachine};
use moor_core::seam::proc::Tool;
use moor_core::user::Identity;

use super::{start, Stop, World};
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
    };
    w.store.claim_machine(w.id, user, &want).unwrap();
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
    };
    let other = super::working_box(&w.store, &w.driver, "other");
    w.store.claim_machine(other, user, &want).unwrap();
    w.store.claim_machine(w.id, user, &want).unwrap();
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
