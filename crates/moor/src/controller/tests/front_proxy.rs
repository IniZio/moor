//! Leases of the front Proxy over a front socket: granted only for the asserting front's own
//! user, only when that user holds every secret the Proxy injects and the Proxy runs, and
//! ended by release or by the next controller start.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::fmt::Write as _;
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

use moor_core::seam::clock::Ts;
use moor_core::user::Identity;
use serde_json::{json, Value};

use super::fronts::{ask, refused};
use super::{config, env_with, Buf};
use crate::controller::{start, Sink};
use crate::Moor;
use sha2::Digest as _;

const TOKEN: &str = "sk-ant-oat01-front-proxy-canary";

/// A served controller whose `slack` front has a Proxy serving the claude rule, and whose
/// `web` front has none.
struct Served {
    dir: tempfile::TempDir,
}

impl Served {
    fn start(dir: tempfile::TempDir) -> Self {
        let cfg = config(
            dir.path(),
            "fronts:\n  slack:\n    proxy_rules: [claude]\n  web: {}\n",
        );
        let running = start(&env_with(dir.path(), &[]), &cfg).expect("start");
        let sink: Sink = Arc::new(Mutex::new(Buf::default()));
        running.serve_in_background(sink);
        Self { dir }
    }

    fn ask(&self, ns: &str, request: &Value) -> Value {
        let socket = self
            .dir
            .path()
            .join("state")
            .join("fronts")
            .join(format!("{ns}.sock"));
        ask(&socket, request)
    }

    /// Stands in for a running Proxy of the slack front.
    fn proxy_up(&self) -> UnixListener {
        let path = Moor::front_proxy_socket(&self.dir.path().join("state"), "slack");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("socket dir");
        }
        let _ = std::fs::remove_file(&path);
        UnixListener::bind(path).expect("bind")
    }

    fn login(&self, identity: &str) {
        self.ask("slack", &json!({"cmd": "enroll", "identity": identity}));
        let set =
            json!({"cmd": "secret-set", "identity": identity, "name": "claude", "value": TOKEN});
        assert_eq!(self.ask("slack", &set)["class"], "ok");
    }

    fn lease(&self, identity: &str) -> Value {
        self.ask(
            "slack",
            &json!({"cmd": "proxy-lease", "identity": identity}),
        )
    }

    fn stored_hashes(&self) -> Vec<String> {
        let db = rusqlite::Connection::open(self.dir.path().join("state").join("moor.db")).unwrap();
        let mut stmt = db.prepare("SELECT token_hash FROM front_leases").unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}

#[test]
fn a_user_without_the_secret_gets_no_lease_but_is_told_which_secret() {
    let served = Served::start(tempfile::tempdir().unwrap());
    let _proxy = served.proxy_up();
    for _ in 0..2 {
        let got = served.lease("slack:T1:U1");
        assert_eq!(
            got["value"],
            json!({"lease": "no_secret", "name": "claude"}),
            "{got}"
        );
        served.ask(
            "slack",
            &json!({"cmd": "enroll", "identity": "slack:T1:U1"}),
        );
    }
    assert!(served.stored_hashes().is_empty());
}

#[test]
fn no_lease_is_granted_while_the_proxy_is_down() {
    let served = Served::start(tempfile::tempdir().unwrap());
    served.login("slack:T1:U1");
    let got = served.lease("slack:T1:U1");
    assert_eq!(got["value"], json!({"lease": "proxy_down"}), "{got}");
    let status = served.ask(
        "slack",
        &json!({"cmd": "proxy-status", "identity": "slack:T1:U1"}),
    );
    assert_eq!(status["value"], json!({"running": false}), "{status}");
    let _proxy = served.proxy_up();
    let status = served.ask(
        "slack",
        &json!({"cmd": "proxy-status", "identity": "slack:T1:U1"}),
    );
    assert_eq!(status["value"], json!({"running": true}), "{status}");
}

#[test]
fn a_lease_is_stored_only_as_a_hash_and_ends_on_release() {
    let served = Served::start(tempfile::tempdir().unwrap());
    let _proxy = served.proxy_up();
    served.login("slack:T1:U1");
    let got = served.lease("slack:T1:U1");
    assert_eq!(got["value"]["lease"], "granted", "{got}");
    let token = got["value"]["token"].as_str().unwrap().to_owned();
    assert!(
        !got.to_string().contains(TOKEN),
        "the reply holds no secret: {got}"
    );
    let hashed = sha2::Sha256::digest(token.as_bytes())
        .iter()
        .fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        });
    assert_eq!(served.stored_hashes(), [hashed]);
    let release = |identity: &str| {
        let ask = json!({"cmd": "proxy-release", "identity": identity, "token": token});
        served.ask("slack", &ask)["value"]["released"].clone()
    };
    served.login("slack:T1:U2");
    assert_eq!(release("slack:T1:U2"), false, "another user cannot end it");
    assert_eq!(release("slack:T1:U1"), true);
    assert_eq!(release("slack:T1:U1"), false);
    assert!(served.stored_hashes().is_empty());
}

#[test]
fn only_a_front_with_proxy_rules_leases_and_only_for_its_own_namespace() {
    let served = Served::start(tempfile::tempdir().unwrap());
    let _proxy = served.proxy_up();
    let got = served.ask("web", &json!({"cmd": "proxy-lease", "identity": "web:x"}));
    refused(&got, "this front has no Proxy");
    let got = served.lease("web:x");
    refused(&got, "may assert only slack:");
}

#[test]
fn a_controller_that_takes_the_state_lock_ends_every_lease_and_one_refused_ends_none() {
    let served = Served::start(tempfile::tempdir().unwrap());
    let _proxy = served.proxy_up();
    served.login("slack:T1:U1");
    assert_eq!(served.lease("slack:T1:U1")["value"]["lease"], "granted");
    let config = served.dir.path().join("controller.yaml");
    let second = start(&env_with(served.dir.path(), &[]), &config);
    assert!(second.is_err(), "the running controller holds the lock");
    assert_eq!(
        served.stored_hashes().len(),
        1,
        "a refused start ends no lease"
    );
    crate::Moor::end_front_leases(&env_with(served.dir.path(), &[])).unwrap();
    assert!(served.stored_hashes().is_empty());
}

#[test]
fn a_controller_start_ends_the_leases_the_last_one_left() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let clock = Arc::new(crate::testing::clock::FakeClock::new(Ts::from_millis(
        1_000,
    )));
    let store = crate::store::Store::open(&state.join("moor.db"), clock).unwrap();
    let (user, _) = store
        .enroll(&Identity::parse("slack:T1:U1").unwrap())
        .unwrap();
    store
        .add_front_lease(b"left", "slack", user, Ts::from_millis(i64::MAX))
        .unwrap();
    let served = Served::start(dir);
    assert!(served.stored_hashes().is_empty());
}
