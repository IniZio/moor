use std::collections::BTreeSet;

use super::{machine_name, plan, Machine, Step};
use crate::via::{Link, Via};

const EXE: &str = "/usr/bin/moor-hc";

fn server() -> Link {
    Link::Server("https://moor.example.com".to_owned())
}

fn via(box_name: &str) -> Via {
    Via {
        exe: EXE.to_owned(),
        link: server(),
        box_name: box_name.to_owned(),
    }
}

fn ours(name: &str, box_name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        via: Some(via(box_name)),
    }
}

fn theirs(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        via: None,
    }
}

fn boxes(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|&n| n.to_owned()).collect()
}

fn add(name: &str, box_name: &str) -> Step {
    Step::Add {
        name: name.to_owned(),
        via: via(box_name),
    }
}

fn remove(name: &str) -> Step {
    Step::Remove {
        name: name.to_owned(),
    }
}

#[test]
fn each_up_box_gets_a_machine_named_after_it() {
    let steps = plan(EXE, &server(), &boxes(&["moor/a", "moor/b"]), &[]);
    assert_eq!(steps, [add("moor.a", "moor/a"), add("moor.b", "moor/b")]);
}

#[test]
fn a_synced_set_plans_nothing() {
    let machines = [ours("moor.a", "moor/a"), theirs("vps")];
    assert_eq!(plan(EXE, &server(), &boxes(&["moor/a"]), &machines), []);
}

#[test]
fn a_machine_for_a_box_no_longer_up_is_removed() {
    let machines = [ours("moor.a", "moor/a"), ours("moor.b", "moor/b")];
    let steps = plan(EXE, &server(), &boxes(&["moor/b"]), &machines);
    assert_eq!(steps, [remove("moor.a")]);
}

#[test]
fn machines_moor_hc_did_not_add_are_never_removed() {
    let other_server = Machine {
        via: Some(Via {
            link: Link::Server("https://other.example.com".to_owned()),
            ..via("moor/a")
        }),
        ..theirs("moor.a")
    };
    let machines = [theirs("vps"), theirs("laptop"), other_server];
    assert!(plan(EXE, &server(), &boxes(&[]), &machines).is_empty());
}

#[test]
fn a_name_another_machine_holds_gets_a_hashed_one() {
    let steps = plan(EXE, &server(), &boxes(&["moor/a"]), &[theirs("moor.a")]);
    let name = steps
        .iter()
        .find_map(|s| match s {
            Step::Add { name, .. } => Some(name.clone()),
            Step::Remove { .. } => None,
        })
        .unwrap();
    assert_eq!(steps, [add(&name, "moor/a")]);
    assert!(
        name.starts_with("moor.a-") && name.len() == "moor.a-".len() + 6,
        "{name}"
    );
}

#[test]
fn a_machine_on_an_old_binary_is_replaced() {
    let old = Machine {
        via: Some(Via {
            exe: "/old/moor-hc".to_owned(),
            ..via("moor/a")
        }),
        ..theirs("moor.a")
    };
    let steps = plan(EXE, &server(), &boxes(&["moor/a"]), &[old]);
    assert_eq!(steps, [remove("moor.a"), add("moor.a", "moor/a")]);
}

#[test]
fn a_second_machine_for_the_same_box_is_removed() {
    let machines = [ours("moor.a", "moor/a"), ours("moor.a-2", "moor/a")];
    let steps = plan(EXE, &server(), &boxes(&["moor/a"]), &machines);
    assert_eq!(steps, [remove("moor.a-2")]);
}

#[test]
fn names_fit_hc_and_differ_for_different_boxes() {
    let none = BTreeSet::new();
    let long_a = format!("{}/{}", "r".repeat(40), "a".repeat(40));
    let long_b = format!("{}/{}", "r".repeat(40), "a".repeat(39));
    let (a, b) = (
        machine_name(&server(), &long_a, &none),
        machine_name(&server(), &long_b, &none),
    );
    assert_ne!(a, b);
    for name in [&a, &b] {
        assert!(name.len() <= 32, "{name}");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c)),
            "{name}"
        );
    }
    assert_eq!(a, machine_name(&server(), &long_a, &none), "stable");
}

#[test]
fn a_bare_name_shaped_like_an_hc_id_is_hashed() {
    let name = machine_name(&server(), "t7", &BTreeSet::new());
    assert!(name.starts_with("t7-"), "{name}");
}
