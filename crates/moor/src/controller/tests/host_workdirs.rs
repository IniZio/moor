//! Where a host Session runs: the operator names the directory; for anyone else the
//! controller gives each Session one of its own, under its state, per user.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::authz::{acting, World, A, B};
use super::fronts::{ask, refused};
use crate::ops::host_session::WORKDIR_OPERATOR_ONLY;

/// The user id `identity` holds on `world`'s slack socket.
fn user_of(world: &World, identity: &str) -> i64 {
    let who = ask(&world.slack(), &acting(identity, &json!({"cmd": "whoami"})));
    who["value"]["user"].as_i64().expect("user")
}

/// The workdir of a host Session `identity` opens with no workdir.
fn opened_by(world: &World, identity: &str) -> (i64, PathBuf) {
    let opened = ask(
        &world.slack(),
        &acting(identity, &json!({"cmd": "open-host-session"})),
    );
    let session = opened["value"]["session"].as_i64().expect("session");
    let workdir = opened["value"]["workdir"].as_str().expect("workdir");
    (session, PathBuf::from(workdir))
}

/// The permission bits of `path`.
fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

#[test]
fn the_operator_opens_a_host_session_at_any_absolute_path() {
    let world = World::new();
    let open = json!({"cmd": "open-host-session", "workdir": "/srv/anywhere"});
    let opened = ask(&world.served.main, &open);
    assert_eq!(opened["value"]["workdir"], "/srv/anywhere", "{opened}");
}

#[test]
fn another_user_naming_a_workdir_is_refused_and_nothing_opens() {
    let world = World::new();
    let open = json!({"cmd": "open-host-session", "workdir": "/home/op"});
    refused(
        &ask(&world.slack(), &acting(A, &open)),
        WORKDIR_OPERATOR_ONLY,
    );
    let listed: Value = ask(&world.served.main, &json!({"cmd": "sessions"}));
    let named = listed["value"]["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .any(|s| s["workdir"] == "/home/op");
    assert!(!named, "{listed}");
}

#[test]
fn a_user_with_no_workdir_gets_an_owner_only_dir_of_its_own_under_the_state() {
    let world = World::new();
    let (session, workdir) = opened_by(&world, A);
    let users = world.served.dir.path().join("state").join("users");
    let mine = users.join(user_of(&world, A).to_string());
    assert_eq!(workdir, mine.join("sessions").join(session.to_string()));
    for dir in [&mine, &mine.join("sessions"), &workdir] {
        assert!(dir.is_dir(), "{}", dir.display());
        assert_eq!(mode(dir), 0o700, "{}", dir.display());
    }
}

#[test]
fn two_users_and_two_sessions_never_share_a_workdir() {
    let world = World::new();
    let (_, a1) = opened_by(&world, A);
    let (_, a2) = opened_by(&world, A);
    let (_, b) = opened_by(&world, B);
    assert_ne!(a1, a2);
    let a_user = a1.parent().and_then(Path::parent).expect("A's dir");
    let b_user = b.parent().and_then(Path::parent).expect("B's dir");
    assert_ne!(a_user, b_user);
    assert!(!b.starts_with(a_user) && !a1.starts_with(b_user));
}

/// The Session `identity` gets from opening with `key` and no workdir.
fn keyed(world: &World, identity: &str, key: &str) -> i64 {
    let open = json!({"cmd": "open-host-session", "key": key});
    let opened = ask(&world.slack(), &acting(identity, &open));
    opened["value"]["session"].as_i64().expect("session")
}

#[test]
fn the_same_key_twice_returns_the_same_live_session() {
    let world = World::new();
    let first = keyed(&world, A, "C1/1.0");
    assert_eq!(keyed(&world, A, "C1/1.0"), first);
    assert_ne!(keyed(&world, A, "C1/2.0"), first);
    let open = json!({"cmd": "open-host-session", "workdir": "/srv/op", "key": "k"});
    let op = |reply: Value| reply["value"]["session"].as_i64().expect("session");
    let main = &world.served.main;
    assert_eq!(op(ask(main, &open)), op(ask(main, &open)));
}

#[test]
fn another_users_same_key_opens_a_separate_session() {
    let world = World::new();
    assert_ne!(keyed(&world, A, "C1/1.0"), keyed(&world, B, "C1/1.0"));
}

#[test]
fn an_ended_sessions_key_opens_a_new_one() {
    let world = World::new();
    let first = keyed(&world, A, "C1/1.0");
    let end = json!({"cmd": "end-host-session", "session": first});
    assert_eq!(ask(&world.slack(), &acting(A, &end))["class"], "ok");
    let second = keyed(&world, A, "C1/1.0");
    assert_ne!(second, first);
    assert_eq!(keyed(&world, A, "C1/1.0"), second);
}

#[test]
fn a_key_out_of_bounds_is_refused() {
    let world = World::new();
    for key in [String::new(), "k".repeat(257)] {
        let open = json!({"cmd": "open-host-session", "key": key});
        let got = ask(&world.slack(), &acting(A, &open));
        assert_eq!(got["class"], "usage", "{got}");
    }
}
