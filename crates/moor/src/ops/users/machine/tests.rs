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
        capacity: 1,
        vm: None,
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
        &add("mac", "dev@mac.local:2222", &["macos", "ios-sim", "macos"]),
    );
    assert_eq!(added.unwrap(), json!({"name": "mac", "changed": true}));
    as_user(&f, "slack:T1:UA", &add("amd", "ci@10.0.0.2", &[])).unwrap();

    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(
        listed,
        json!({"machines": [
            {"name": "amd", "ssh": "ci@10.0.0.2", "labels": [], "capacity": 1, "kind": "physical"},
            {"name": "mac", "ssh": "dev@mac.local:2222", "labels": ["ios-sim", "macos"], "capacity": 1, "kind": "physical"},
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
        json!({"machines": [{"name": "mac", "ssh": "dev@new.local:22", "labels": ["ios-sim"], "capacity": 1, "kind": "physical"}]})
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
fn capacity_is_stored_and_listed_and_zero_is_refused() {
    let f = fixture();
    enroll(&f, "slack:T1:U1");
    let mut cmd = add("mac", "dev@mac.local", &["macos"]);
    if let UserCmd::MachineAdd { capacity, .. } = &mut cmd {
        *capacity = 3;
    }
    as_user(&f, "slack:T1:U1", &cmd).unwrap();
    let list = as_user(&f, "slack:T1:U1", &UserCmd::MachineList).unwrap();
    assert_eq!(list["machines"][0]["capacity"], 3);
    if let UserCmd::MachineAdd { capacity, .. } = &mut cmd {
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

fn add_vm(name: &str, vm: &str) -> UserCmd {
    UserCmd::MachineAdd {
        name: name.to_owned(),
        ssh: "me@box:22".to_owned(),
        labels: vec!["android-emu".to_owned()],
        capacity: 3,
        vm: Some(vm.to_owned()),
    }
}

#[test]
fn a_vm_host_is_stored_and_listed_with_its_kind_and_provider() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    as_user(&f, "slack:T1:UA", &add_vm("hv", "external")).unwrap();
    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(
        listed,
        json!({"machines": [{
            "name": "hv", "ssh": "me@box:22", "labels": ["android-emu"], "capacity": 3,
            "kind": "vm-host", "vm_provider": "external"
        }]})
    );
    let again = as_user(&f, "slack:T1:UA", &add("hv", "me@box:22", &[])).unwrap();
    assert_eq!(again, json!({"name": "hv", "changed": true}));
    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(listed["machines"][0]["kind"], "physical");
    assert!(listed["machines"][0].get("vm_provider").is_none());
}

#[test]
fn a_bad_vm_provider_is_refused_naming_the_flag() {
    let f = fixture();
    enroll(&f, "slack:T1:UA");
    for bad in ["Ext", "9x", "a_b", "", "has space"] {
        let err = as_user(&f, "slack:T1:UA", &add_vm("hv", bad)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage, "{bad:?}");
        let text = err.error.to_string();
        assert!(text.contains("--vm"), "{bad:?}: {text}");
    }
    let long = "a".repeat(65);
    assert!(as_user(&f, "slack:T1:UA", &add_vm("hv", &long)).is_err());
    let listed = as_user(&f, "slack:T1:UA", &UserCmd::MachineList).unwrap();
    assert_eq!(listed["machines"].as_array().unwrap().len(), 0);
}
