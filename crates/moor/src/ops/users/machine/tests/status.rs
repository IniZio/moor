#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests"
)]

use std::sync::{Arc, Mutex};

use moor_client::link::status::Status;
use moor_core::request::{MachineLink, UserCmd};
use moor_core::seam::clock::Ts;
use moor_core::ErrorKind;

use crate::cx::MachineLinks;
use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id, Fixture};

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn status_cmd(name: &str) -> UserCmd {
    UserCmd::MachineStatus {
        name: name.to_owned(),
    }
}

/// Enrolls `name` for `who`, records its link, and returns its id.
fn linked(f: &Fixture, who: &str, name: &str, online: bool) -> i64 {
    let enroll = UserCmd::MachineEnroll {
        email: None,
        name: name.to_owned(),
        pubkey: KEY.to_owned(),
        labels: Vec::new(),
        capacity: 1,
        pool: None,
    };
    let made = run(&f.cx, &f.vault, &id(who), &enroll).unwrap();
    let machine_id = made["machine_id"].as_i64().unwrap();
    f.cx.store()
        .unwrap()
        .set_machine_links(&[MachineLink {
            machine_id,
            online,
            since: Ts::from_millis(1_000_000),
            version: Some("1.2.3".into()),
            protocol: Some(2),
            max_boxes: None,
            cache_gb: None,
            features: Vec::new(),
        }])
        .unwrap();
    machine_id
}

fn sample() -> Status {
    Status {
        version: "1.2.3".into(),
        protocol: 2,
        os: "linux".into(),
        arch: "x86_64".into(),
        free_disk_gb: Some(40),
        pool: None,
        recent: Vec::new(),
    }
}

/// Machine links that answer every status with [`sample`] and note whom they asked.
#[derive(Default)]
struct Asked(Mutex<Vec<i64>>);

impl MachineLinks for Asked {
    fn status(&self, machine_id: i64) -> Result<Status, String> {
        self.0.lock().unwrap().push(machine_id);
        Ok(sample())
    }
}

#[test]
fn the_owner_gets_the_agents_status() {
    let mut f = fixture();
    let asked = Arc::new(Asked::default());
    f.cx = f.cx.with_machine_links(Arc::clone(&asked) as _);
    let machine_id = linked(&f, "authgear:a", "minion", true);
    let got = run(&f.cx, &f.vault, &id("authgear:a"), &status_cmd("minion")).unwrap();
    assert_eq!(got["version"], "1.2.3");
    assert_eq!(got["free_disk_gb"], 40);
    assert_eq!(*asked.0.lock().unwrap(), vec![machine_id]);
}

#[test]
fn another_users_machine_is_as_unknown_as_a_missing_one() {
    let f = fixture();
    linked(&f, "authgear:a", "minion", true);
    f.cx.store()
        .unwrap()
        .enroll(&id("authgear:b"), None)
        .unwrap();
    let other = run(&f.cx, &f.vault, &id("authgear:b"), &status_cmd("minion")).unwrap_err();
    let missing = run(&f.cx, &f.vault, &id("authgear:b"), &status_cmd("nope")).unwrap_err();
    assert_eq!(other.kind, ErrorKind::Refused);
    assert_eq!(other.error.to_string(), "you have no machine named minion");
    assert_eq!(missing.error.to_string(), "you have no machine named nope");
}

#[test]
fn an_offline_machine_is_refused_with_since_when() {
    let f = fixture();
    linked(&f, "authgear:a", "minion", false);
    let err = run(&f.cx, &f.vault, &id("authgear:a"), &status_cmd("minion")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(
        err.error.to_string(),
        "machine minion is offline since 1970-01-01T00:16:40Z"
    );
}
