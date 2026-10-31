#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::request::UserCmd;
use moor_core::ErrorKind;
use serde_json::json;

use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id, Fixture};

fn add(name: &str, labels: &[&str]) -> UserCmd {
    UserCmd::MachineEnroll {
        email: None,
        name: name.to_owned(),
        pubkey: KEY.to_owned(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        capacity: 1,
        pool: None,
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
    f.cx.store().unwrap().enroll(&id(who), None).unwrap();
}

#[test]
fn add_list_and_remove() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    let added = as_user(
        &f,
        "slack:T1:UA",
        &add("mac", &["macos", "ios-sim", "macos"]),
    )
    .unwrap();
    assert_eq!(added["name"], "mac", "{added}");
    as_user(&f, "slack:T1:UA", &add("amd", &[])).unwrap();

    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(
        listed,
        json!({"machines": [
            {"name": "amd", "labels": [], "capacity": 1},
            {"name": "mac", "labels": ["ios-sim", "macos"], "capacity": 1},
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
    as_user(&f, "slack:T1:UA", &add("mac", &["macos"])).unwrap();
    as_user(&f, "slack:T1:UA", &add("mac", &["ios-sim"])).unwrap();
    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(
        listed,
        json!({"machines": [{"name": "mac", "labels": ["ios-sim"], "capacity": 1}]})
    );
}

#[test]
fn a_users_machines_are_theirs_alone() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    enroll(&f, "slack:T1:UB");
    as_user(&f, "slack:T1:UA", &add("mac", &["macos"])).unwrap();

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

    as_user(&f, "slack:T1:UB", &add("mac", &[])).unwrap();
    let mine = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(
        mine["machines"][0]["labels"],
        json!(["macos"]),
        "B's add never replaces A's"
    );
}

#[test]
fn a_bad_machine_is_a_usage_error_and_stores_nothing() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    for cmd in [add("Mac", &[]), add("mac", &["Bad"])] {
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
fn capacity_is_stored_and_listed_and_zero_is_refused() {
    let f = fixture();
    enroll(&f, "slack:T1:U1");
    let mut cmd = add("mac", &["macos"]);
    if let UserCmd::MachineEnroll { capacity, .. } = &mut cmd {
        *capacity = 3;
    }
    as_user(&f, "slack:T1:U1", &cmd).unwrap();
    let list = as_user(&f, "slack:T1:U1", &UserCmd::MachineList).unwrap();
    assert_eq!(list["machines"][0]["capacity"], 3);
    if let UserCmd::MachineEnroll { capacity, .. } = &mut cmd {
        *capacity = 0;
    }
    let err = as_user(&f, "slack:T1:U1", &cmd).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn machine_enroll(email: Option<&str>, name: &str, pubkey: &str, capacity: u16) -> UserCmd {
    UserCmd::MachineEnroll {
        email: email.map(|e| moor_core::user::Email::parse(e).unwrap()),
        name: name.to_owned(),
        pubkey: pubkey.to_owned(),
        labels: vec!["macos".to_owned()],
        capacity,
        pool: None,
    }
}

#[test]
fn machine_enroll_joins_the_user_with_the_email_and_keeps_the_id_on_re_enroll() {
    let f = fixture();
    let slack = f.cx.store().unwrap();
    let email = moor_core::user::Email::parse("alice@example.com").unwrap();
    let (alice, _) = slack.enroll(&id("slack:T1:UA"), Some(&email)).unwrap();
    let cmd = machine_enroll(Some("alice@example.com"), "minion", KEY, 2);
    let first = as_user(&f, "authgear:alice-sub", &cmd).unwrap();
    assert_eq!(first["user"], json!(alice.get()), "{first}");
    assert_eq!(first["owner"], "alice@example.com");
    assert_eq!(first["capacity"], 2);
    let again = as_user(&f, "authgear:alice-sub", &cmd).unwrap();
    assert_eq!(again["machine_id"], first["machine_id"]);
}

#[test]
fn machine_enroll_refuses_a_key_that_is_not_one_ed25519_public_key_and_a_bad_name() {
    let f = fixture();
    for (name, pubkey, capacity) in [
        ("minion", "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ", 1),
        ("minion", &format!("{KEY} alice@laptop") as &str, 1),
        ("minion", KEY.strip_suffix('l').unwrap(), 1),
        ("Minion", KEY, 1),
        ("minion", KEY, 0),
    ] {
        let err = as_user(
            &f,
            "authgear:a",
            &machine_enroll(None, name, pubkey, capacity),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage, "{name} {pubkey} {capacity}");
    }
    assert_eq!(
        f.cx.store().unwrap().user_of(&id("authgear:a")).unwrap(),
        None,
        "no user made"
    );
}

#[test]
fn a_pool_provider_is_stored_listed_and_refused_when_malformed() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    let mut cmd = add("hv", &["android-emu"]);
    if let UserCmd::MachineEnroll { pool, .. } = &mut cmd {
        *pool = Some("cloudhypervisor".to_owned());
    }
    as_user(&f, "slack:T1:UA", &cmd).unwrap();
    as_user(&f, "slack:T1:UA", &add("mac", &[])).unwrap();
    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(listed["machines"][0]["pool"], "cloudhypervisor");
    assert!(listed["machines"][1].get("pool").is_none());
    if let UserCmd::MachineEnroll { pool, .. } = &mut cmd {
        *pool = Some("Cloud_Hypervisor".to_owned());
    }
    let err = as_user(&f, "slack:T1:UA", &cmd).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(err.error.to_string().contains("--runtime"), "{err:?}");
}

#[test]
fn an_enrolled_machine_keeps_its_pool_provider() {
    let f = fixture();
    let mut cmd = machine_enroll(Some("alice@example.com"), "hv1", KEY, 2);
    if let UserCmd::MachineEnroll { pool, .. } = &mut cmd {
        *pool = Some("cloudhypervisor".to_owned());
    }
    as_user(&f, "authgear:alice-sub", &cmd).unwrap();
    let listed = as_user(&f, "authgear:alice-sub", &UserCmd::MachineList).unwrap();
    assert_eq!(listed["machines"][0]["name"], "hv1");
    assert_eq!(listed["machines"][0]["pool"], "cloudhypervisor");
    if let UserCmd::MachineEnroll { pool, .. } = &mut cmd {
        *pool = Some("9bad".to_owned());
    }
    let err = as_user(&f, "authgear:alice-sub", &cmd).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

mod status;
