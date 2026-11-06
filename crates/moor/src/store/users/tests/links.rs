//! Which runner a machine's link routes to, as runners report, drain and go quiet.

use std::time::Duration;

use moor_core::config::EnrolledMachine;
use moor_core::request::{MachineLink, RunnerAddr};
use moor_core::seam::clock::Ts;
use moor_core::user::Identity;

use crate::store::tests::open;
use crate::store::Store;

const KEYS: [&str; 2] = [
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
];

fn link(machine_id: i64, online: bool, since: i64) -> MachineLink {
    MachineLink {
        machine_id,
        online,
        since: Ts::from_millis(since),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: Vec::new(),
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        warm_starts: None,
        memory: None,
    }
}

fn at(secs: u64) -> Ts {
    Ts::from_millis(1_000_000).saturating_add(Duration::from_secs(secs))
}

/// Alice's machines `one` and `two`, by id.
fn two_machines(store: &Store) -> [i64; 2] {
    let (alice, _) = store
        .enroll(&Identity::parse("authgear:a").unwrap(), None)
        .unwrap();
    let [a, b] = KEYS;
    [("one", a), ("two", b)].map(|(name, key)| {
        let m = EnrolledMachine::parse(name, key, &[], 1).unwrap();
        store.enroll_machine(alice, &m).unwrap()
    })
}

fn report(store: &Store, runner: &RunnerAddr, draining: bool, links: &[MachineLink], now: Ts) {
    store
        .set_machine_links(runner, draining, links, now)
        .unwrap();
}

#[test]
fn a_newer_link_on_another_runner_takes_the_machine_and_the_old_runner_cannot_take_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let [one, _] = two_machines(&store);
    let (a, b) = (
        RunnerAddr::new("10.0.0.1:7701"),
        RunnerAddr::new("10.0.0.2:7701"),
    );
    report(&store, &a, false, &[link(one, true, 5)], at(0));
    assert_eq!(store.machine_runner(one, at(0)).unwrap(), Some(a.clone()));
    report(&store, &b, false, &[link(one, true, 9)], at(1));
    assert_eq!(store.machine_runner(one, at(1)).unwrap(), Some(b.clone()));
    report(&store, &a, false, &[link(one, false, 10)], at(2));
    report(&store, &a, false, &[link(one, true, 5)], at(3));
    assert_eq!(store.machine_runner(one, at(3)).unwrap(), Some(b));
}

#[test]
fn a_runner_that_stops_listing_a_machine_drops_only_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let [one, two] = two_machines(&store);
    let (a, b) = (
        RunnerAddr::new("10.0.0.1:7701"),
        RunnerAddr::new("10.0.0.2:7701"),
    );
    report(&store, &a, false, &[link(one, true, 5)], at(0));
    report(&store, &b, false, &[link(two, true, 5)], at(0));
    report(&store, &a, false, &[], at(1));
    assert_eq!(store.machine_runner(one, at(1)).unwrap(), None);
    assert_eq!(store.machine_runner(two, at(1)).unwrap(), Some(b));
}

#[test]
fn a_draining_runners_links_route_nowhere_until_another_runner_takes_them() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let [one, _] = two_machines(&store);
    let (a, b) = (
        RunnerAddr::new("10.0.0.1:7701"),
        RunnerAddr::new("10.0.0.2:7701"),
    );
    report(&store, &a, true, &[link(one, true, 5)], at(0));
    assert_eq!(store.machine_runner(one, at(0)).unwrap(), None);
    report(&store, &b, false, &[link(one, true, 3)], at(1));
    assert_eq!(store.machine_runner(one, at(1)).unwrap(), Some(b));
}

#[test]
fn a_link_unheard_for_over_a_minute_routes_nowhere_until_its_runner_reports_again() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let [one, _] = two_machines(&store);
    let a = RunnerAddr::new("/run/moor/runner/runner.sock");
    report(&store, &a, false, &[link(one, true, 5)], at(0));
    assert_eq!(store.machine_runner(one, at(60)).unwrap(), Some(a.clone()));
    assert_eq!(store.machine_runner(one, at(61)).unwrap(), None);
    report(&store, &a, false, &[link(one, true, 5)], at(61));
    assert_eq!(store.machine_runner(one, at(61)).unwrap(), Some(a));
}
