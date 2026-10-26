use std::collections::BTreeMap;

use super::{pick_machine, MachinePick};
use crate::config::UserMachine;

fn machine(name: &str, labels: &[&str]) -> UserMachine {
    let labels: Vec<String> = labels.iter().map(|l| (*l).to_owned()).collect();
    UserMachine::parse(name, "dev@mac.example", &labels).expect("valid machine")
}

fn want(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|l| (*l).to_owned()).collect()
}

#[test]
fn attaches_the_first_machine_by_name_holding_every_label() {
    let own = [
        machine("air", &["macos"]),
        machine("mini", &["macos", "xcode"]),
        machine("studio", &["macos", "xcode"]),
    ];
    let got = pick_machine(&want(&["xcode", "macos"]), &own, &BTreeMap::new());
    assert_eq!(got, MachinePick::Attach(own[1].clone()));
    assert_eq!(got.note(), None);
}

#[test]
fn no_fitting_machine_names_the_labels() {
    let own = [machine("air", &["macos"])];
    let got = pick_machine(&want(&["macos", "xcode"]), &own, &BTreeMap::new());
    assert_eq!(
        got.note().as_deref(),
        Some("machine left out: you have no machine with labels macos, xcode")
    );
}

#[test]
fn a_held_machine_names_the_box_holding_it() {
    let own = [machine("mini", &["macos"])];
    let held = BTreeMap::from([("mini".to_owned(), "app-1".to_owned())]);
    let got = pick_machine(&want(&["macos"]), &own, &held);
    assert_eq!(
        got.note().as_deref(),
        Some("machine left out: machine mini is attached to Box app-1")
    );
}

#[test]
fn a_held_machine_gives_way_to_the_next_free_one() {
    let own = [machine("mini", &["macos"]), machine("studio", &["macos"])];
    let held = BTreeMap::from([("mini".to_owned(), "app-1".to_owned())]);
    let got = pick_machine(&want(&["macos"]), &own, &held);
    assert_eq!(got, MachinePick::Attach(own[1].clone()));
}
