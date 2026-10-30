#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use moor_client::link::broker::{Ask, Brokered, PoolOp};
use moor_client::link::ops::{self, Reply};
use moor_client::link::status::Status;
use moor_core::request::{MachineLink, UserCmd};
use moor_core::seam::clock::Ts;
use moor_core::ErrorKind;
use tokio::net::UnixListener;

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

/// Serves one broker request on the runner socket under the fixture's state dir and returns
/// the request it saw.
fn broker(f: &Fixture) -> std::thread::JoinHandle<Ask> {
    let dir = f.cx.state_dir().unwrap().join("runner");
    std::fs::create_dir_all(&dir).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(dir.join("links.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = UnixListener::from_std(listener).unwrap();
                let (mut conn, _) = listener.accept().await.unwrap();
                let ask: Ask = ops::read_line(&mut conn).await.unwrap();
                let reply = Brokered::Agent {
                    reply: Reply::Status(Box::new(sample())),
                };
                ops::write_line(&mut conn, &reply).await.unwrap();
                ask
            })
    })
}

#[test]
fn the_owner_gets_the_agents_status() {
    let f = fixture();
    let machine_id = linked(&f, "authgear:a", "minion", true);
    let served = broker(&f);
    let got = run(&f.cx, &f.vault, &id("authgear:a"), &status_cmd("minion")).unwrap();
    assert_eq!(got["version"], "1.2.3");
    assert_eq!(got["free_disk_gb"], 40);
    let ask = served.join().unwrap();
    assert_eq!(ask.machine_id, machine_id);
    assert_eq!(ask.op, PoolOp::Status);
}

#[test]
fn another_users_machine_is_as_unknown_as_a_missing_one() {
    let f = fixture();
    linked(&f, "authgear:a", "minion", true);
    f.cx.store().unwrap().enroll(&id("authgear:b"), None).unwrap();
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
