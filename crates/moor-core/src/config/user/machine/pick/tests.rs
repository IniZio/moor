use std::collections::BTreeMap;

use super::{pick_machine, MachinePick};
use crate::config::{RepoMachine, UserMachine};

fn machine(name: &str, labels: &[&str]) -> UserMachine {
    let labels: Vec<String> = labels.iter().map(|l| (*l).to_owned()).collect();
    UserMachine::parse(name, "dev@mac.example", &labels).expect("valid machine")
}

fn vm_host(name: &str, labels: &[&str]) -> UserMachine {
    machine(name, labels).with_vm_host("external").unwrap()
}

#[test]
fn matches_vm_hosts_and_physical_machines_by_labels_alike() {
    let own = [
        machine("air", &["macos"]),
        vm_host("hv", &["android-emu"]),
        vm_host("hv2", &["macos"]),
    ];
    assert_eq!(
        pick_machine(&want(&["android-emu"], &[8081]), &own, &held(&[])),
        attach(own.get(1).unwrap(), 0, &[8081])
    );
    assert_eq!(
        pick_machine(&want(&["macos"], &[]), &own, &held(&[("air", 0, "a")])),
        attach(own.get(2).unwrap(), 0, &[])
    );
    let full = held(&[("air", 0, "a"), ("hv2", 0, "b")]);
    let pick = pick_machine(&want(&["macos"], &[]), &own, &full);
    assert!(matches!(pick, MachinePick::LeftOut { .. }), "{pick:?}");
}

fn want(labels: &[&str], ports: &[u16]) -> RepoMachine {
    RepoMachine {
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        ports: ports.to_vec(),
        ..RepoMachine::default()
    }
}

fn held(rows: &[(&str, u16, &str)]) -> BTreeMap<String, BTreeMap<u16, String>> {
    let mut out: BTreeMap<String, BTreeMap<u16, String>> = BTreeMap::new();
    for (m, slot, b) in rows {
        out.entry((*m).to_owned())
            .or_default()
            .insert(*slot, (*b).to_owned());
    }
    out
}

fn attach(m: &UserMachine, slot: u16, ports: &[u16]) -> MachinePick {
    MachinePick::Attach {
        machine: m.clone(),
        slot,
        ports: ports.to_vec(),
    }
}

#[test]
fn attaches_the_first_machine_by_name_holding_every_label() {
    let own = [
        machine("air", &["macos"]),
        machine("mini", &["macos", "xcode"]),
        machine("studio", &["macos", "xcode"]),
    ];
    let got = pick_machine(&want(&["xcode", "macos"], &[8081]), &own, &BTreeMap::new());
    assert_eq!(got, attach(&own[1], 0, &[8081]));
    assert_eq!(got.note(), None);
}

#[test]
fn no_fitting_machine_names_the_labels() {
    let own = [machine("air", &["macos"])];
    let got = pick_machine(&want(&["macos", "xcode"], &[]), &own, &BTreeMap::new());
    assert_eq!(
        got.note().as_deref(),
        Some("machine left out: you have no machine with labels macos, xcode")
    );
}

#[test]
fn a_held_machine_names_the_box_holding_it() {
    let own = [machine("mini", &["macos"])];
    let got = pick_machine(&want(&["macos"], &[]), &own, &held(&[("mini", 0, "app-1")]));
    assert_eq!(
        got.note().as_deref(),
        Some("machine left out: machine mini is attached to Box app-1")
    );
}

#[test]
fn a_held_machine_gives_way_to_the_next_free_one() {
    let own = [machine("mini", &["macos"]), machine("studio", &["macos"])];
    let got = pick_machine(&want(&["macos"], &[]), &own, &held(&[("mini", 0, "app-1")]));
    assert_eq!(got, attach(&own[1], 0, &[]));
}

#[test]
fn a_shared_machine_gives_the_lowest_free_slot_with_ports_moved_ten_per_slot() {
    let mini = machine("mini", &["macos"]).with_capacity(3).unwrap();
    let own = [mini.clone()];
    let ports = [8081, 8085];
    let got = pick_machine(&want(&["macos"], &ports), &own, &held(&[("mini", 0, "a")]));
    assert_eq!(got, attach(&mini, 1, &[8091, 8095]));
    let got = pick_machine(&want(&["macos"], &ports), &own, &held(&[("mini", 1, "b")]));
    assert_eq!(got, attach(&mini, 0, &[8081, 8085]));
}

#[test]
fn a_full_shared_machine_names_every_box_holding_it() {
    let own = [machine("mini", &["macos"]).with_capacity(2).unwrap()];
    let rows = held(&[("mini", 0, "a"), ("mini", 1, "b")]);
    let got = pick_machine(&want(&["macos"], &[]), &own, &rows);
    assert_eq!(
        got.note().as_deref(),
        Some("machine left out: machine mini is attached to Boxes a, b")
    );
}

#[test]
fn a_port_moved_past_65535_refuses_the_attach() {
    let own = [machine("mini", &["macos"]).with_capacity(2).unwrap()];
    let rows = held(&[("mini", 0, "a")]);
    let got = pick_machine(&want(&["macos"], &[65530]), &own, &rows);
    assert_eq!(
        got.note().as_deref(),
        Some("machine left out: port 65530 in slot 1 of machine mini would be above 65535")
    );
    let got = pick_machine(&want(&["macos"], &[65525]), &own, &rows);
    assert_eq!(got, attach(&own[0], 1, &[65535]));
}

#[test]
fn capacity_zero_is_refused() {
    assert!(machine("mini", &[]).with_capacity(0).is_err());
}
