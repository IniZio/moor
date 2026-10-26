#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::request::UserCmd;
use moor_core::ErrorKind;
use serde_json::json;

use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id, Fixture};

fn add(name: &str, ssh: &str, labels: &[&str]) -> UserCmd {
    UserCmd::MachineAdd {
        name: name.to_owned(),
        ssh: ssh.to_owned(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
    }
}

fn as_user(
    f: &Fixture,
    who: &str,
    cmd: &UserCmd,
) -> Result<serde_json::Value, crate::error::CommandError> {
    run(&f.cx, &f.vault, &id(who), cmd)
}

fn enroll(f: &Fixture, who: &str) {
    f.cx.store().unwrap().enroll(&id(who)).unwrap();
}

#[test]
fn add_list_and_remove() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    let added = as_user(
        &f,
        "slack:T1:UA",
        &add("mac", "dev@mac.local:2222", &["macos", "ios-sim", "macos"]),
    );
    assert_eq!(added.unwrap(), json!({"name": "mac", "changed": true}));
    as_user(&f, "slack:T1:UA", &add("amd", "ci@10.0.0.2", &[])).unwrap();

    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(
        listed,
        json!({"machines": [
            {"name": "amd", "ssh": "ci@10.0.0.2", "labels": []},
            {"name": "mac", "ssh": "dev@mac.local:2222", "labels": ["ios-sim", "macos"]},
        ]})
    );

    let rm = UserCmd::MachineRemove { name: "mac".into() };
    assert_eq!(
        as_user(&f, "slack:T1:UA", &rm).unwrap(),
        json!({"name": "mac", "changed": true})
    );
    assert_eq!(
        as_user(&f, "slack:T1:UA", &rm).unwrap(),
        json!({"name": "mac", "changed": false})
    );
    let left = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(left["machines"].as_array().unwrap().len(), 1);
}

#[test]
fn adding_the_same_name_replaces_it() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    as_user(&f, "slack:T1:UA", &add("mac", "dev@old.local", &["macos"])).unwrap();
    as_user(
        &f,
        "slack:T1:UA",
        &add("mac", "dev@new.local:22", &["ios-sim"]),
    )
    .unwrap();
    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(
        listed,
        json!({"machines": [{"name": "mac", "ssh": "dev@new.local:22", "labels": ["ios-sim"]}]})
    );
}

#[test]
fn a_users_machines_are_theirs_alone() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    enroll(&f, "slack:T1:UB");
    as_user(&f, "slack:T1:UA", &add("mac", "dev@a.local", &["macos"])).unwrap();

    let theirs = as_user(&f, "slack:T1:UB", &UserCmd::MachineList).unwrap();
    assert_eq!(theirs, json!({"machines": []}), "B never sees A's machine");

    let rm = UserCmd::MachineRemove { name: "mac".into() };
    let removed = as_user(&f, "slack:T1:UB", &rm).unwrap();
    assert_eq!(
        removed["changed"],
        json!(false),
        "B cannot remove A's machine"
    );
    let mine = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(mine["machines"].as_array().unwrap().len(), 1);

    as_user(&f, "slack:T1:UB", &add("mac", "dev@b.local", &[])).unwrap();
    let mine = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(
        mine["machines"][0]["ssh"],
        json!("dev@a.local"),
        "B's add never replaces A's"
    );
}

#[test]
fn a_bad_machine_is_a_usage_error_and_stores_nothing() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    for cmd in [
        add("Mac", "dev@h", &[]),
        add("mac", "-oProxyCommand=x", &[]),
        add("mac", "a b@c", &[]),
        add("mac", "root@host;rm", &[]),
        add("mac", "dev@h", &["Bad"]),
    ] {
        let err = as_user(&f, "slack:T1:UA", &cmd).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage, "{cmd:?}");
    }
    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(listed, json!({"machines": []}));
}

#[test]
fn a_caller_without_a_user_is_refused() {
    let f = fixture();
    let err = as_user(&f, "slack:T1:UZ", &UserCmd::MachineList).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[test]
fn a_box_gets_only_its_requesters_machine() {
    use crate::ops::common::machine::choose;
    use moor_core::config::{MachinePick, RepoMachine};
    use std::collections::BTreeMap;

    let f = fixture();
    let (me, _) = f.cx.store().unwrap().enroll(&id("slack:T1:UA")).unwrap();
    enroll(&f, "slack:T1:UB");
    as_user(&f, "slack:T1:UA", &add("mini", "me@mini", &["macos"])).unwrap();
    as_user(&f, "slack:T1:UB", &add("air", "them@air", &["macos"])).unwrap();
    let want = RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
    };
    let got = choose(f.cx.store().unwrap(), me, &want, &BTreeMap::new()).unwrap();
    let attached = match got {
        MachinePick::Attach(m) => Some((m.name, m.ssh)),
        MachinePick::LeftOut { .. } => None,
    };
    assert_eq!(attached, Some(("mini".to_owned(), "me@mini".to_owned())));
}
