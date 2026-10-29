//! Front sockets and the controller socket, each speaking only for its own identities.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use moor_core::controller::PROTOCOL;
use serde_json::{json, Value};

use super::{config, env_with, Buf};
use crate::controller::{start, Sink};

/// A running controller with a `slack` front, and where its sockets and stderr are.
pub(super) struct Served {
    /// Holds the config, the vault key and the state dir.
    pub(super) dir: tempfile::TempDir,
    /// The controller socket.
    pub(super) main: PathBuf,
    /// Everything the controller logged.
    pub(super) log: Buf,
}

impl Served {
    /// Starts a controller with the `slack` and `web` fronts.
    pub(super) fn start() -> Self {
        Self::start_in(tempfile::tempdir().expect("tmp"), &[])
    }

    /// [`Self::start`] with its config and state in `dir`, and the variables `extra` set.
    pub(super) fn start_in(dir: tempfile::TempDir, extra: &[(&str, &str)]) -> Self {
        Self::start_fronts(dir, "fronts:\n  slack: {}\n  web: {}\n", extra)
    }

    /// [`Self::start_in`] with the controller config `cfg`.
    pub(super) fn start_fronts(dir: tempfile::TempDir, cfg: &str, extra: &[(&str, &str)]) -> Self {
        let cfg = config(dir.path(), cfg);
        let running = start(&env_with(dir.path(), extra), &cfg).expect("start");
        let main = running.socket.clone();
        let log = Buf::default();
        let sink: Sink = Arc::new(Mutex::new(log.clone()));
        running.serve_in_background(sink);
        Self { dir, main, log }
    }

    /// The socket of front `ns`.
    pub(super) fn front(&self, ns: &str) -> PathBuf {
        self.dir
            .path()
            .join("state")
            .join("fronts")
            .join(ns)
            .join("front")
    }
}

/// Sends `request` on a fresh connection to `socket` and returns its response envelope.
pub(super) fn ask(socket: &Path, request: &Value) -> Value {
    let mut write = UnixStream::connect(socket).expect("connect");
    let hello = json!({"frame": "hello", "protocol": PROTOCOL});
    let frame = json!({"frame": "request", "id": "1", "request": request});
    writeln!(write, "{hello}\n{frame}").expect("send");
    let mut read = BufReader::new(write);
    let mut line = String::new();
    loop {
        line.clear();
        read.read_line(&mut line).expect("frame");
        let got: Value = serde_json::from_str(&line).expect("json frame");
        if got["frame"] == "response" {
            return got["envelope"].clone();
        }
        if got["frame"] == "error" {
            return got;
        }
    }
}

/// This process's effective uid, as the controller socket sees its peer.
pub(super) fn me() -> String {
    format!("local:{}", rustix::process::geteuid().as_raw())
}

/// Asserts `envelope` is a refusal whose message contains `want`.
pub(super) fn refused(envelope: &Value, want: &str) {
    assert_eq!(envelope["class"], "refused", "{envelope}");
    assert_eq!(envelope["error"]["kind"], "refused", "{envelope}");
    let message = envelope["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(want), "{envelope}");
}

#[test]
fn each_front_gets_an_owner_only_socket_in_an_owner_only_dir() {
    let served = Served::start();
    let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode(&served.dir.path().join("state").join("fronts")), 0o700);
    for ns in ["slack", "web"] {
        assert_eq!(mode(&served.front(ns)), 0o600, "{ns}");
        assert_eq!(mode(served.front(ns).parent().unwrap()), 0o700, "{ns}");
    }
}

#[test]
fn the_controller_socket_acts_as_its_peer_uid() {
    let served = Served::start();
    let got = ask(&served.main, &json!({"cmd": "whoami"}));
    assert_eq!(got["value"]["identity"], me(), "{got}");
    let got = ask(&served.main, &json!({"cmd": "whoami", "identity": me()}));
    assert_eq!(got["value"]["identity"], me(), "{got}");
}

#[test]
fn the_controller_socket_refuses_a_front_identity() {
    let served = Served::start();
    for other in ["slack:T1:U1", "local:4242"] {
        let got = ask(&served.main, &json!({"cmd": "whoami", "identity": other}));
        refused(&got, "speaks only for local:");
    }
}

#[test]
fn a_front_socket_acts_only_in_its_own_namespace() {
    let served = Served::start();
    let slack = served.front("slack");
    let got = ask(&slack, &json!({"cmd": "whoami", "identity": "slack:T1:U1"}));
    assert_eq!(got["value"]["identity"], "slack:T1:U1", "{got}");
    for other in [me(), "web:alice".to_owned()] {
        let got = ask(&slack, &json!({"cmd": "whoami", "identity": other}));
        refused(&got, "may assert only slack:");
    }
    refused(
        &ask(&slack, &json!({"cmd": "whoami"})),
        "must assert an identity",
    );
    let got = ask(
        &served.front("web"),
        &json!({"cmd": "whoami", "identity": "web:alice"}),
    );
    assert_eq!(got["value"]["identity"], "web:alice", "{got}");
}

#[test]
fn a_front_socket_serves_the_other_requests_too() {
    let served = Served::start();
    let got = ask(&served.front("slack"), &json!({"cmd": "version"}));
    assert_eq!(got["class"], "ok", "{got}");
}

#[test]
fn a_start_acts_only_as_an_identity_its_socket_may_assert() {
    let served = Served::start();
    let start = |identity: Option<&str>| {
        let mut req = json!({"cmd": "start", "name": "demo"});
        if let Some(id) = identity {
            req["identity"] = json!(id);
        }
        req
    };
    refused(
        &ask(&served.main, &start(Some("local:4242"))),
        "speaks only for local:",
    );
    let slack = served.front("slack");
    refused(
        &ask(&slack, &start(Some("web:alice"))),
        "may assert only slack:",
    );
    refused(&ask(&slack, &start(None)), "must assert an identity");
    for (socket, identity) in [(&slack, Some("slack:T1:U1")), (&served.main, None)] {
        let got = ask(socket, &start(identity));
        refused(&got, "log in first: moor user enroll");
        let who = identity.map_or_else(me, str::to_owned);
        assert_eq!(got["value"]["identity"], who, "{got}");
    }
}

#[test]
fn env_reads_a_repo_as_the_identity_a_start_acts_as() {
    let served = Served::start();
    let env = json!({"cmd": "env", "repo": "o/n"});
    let start = json!({"cmd": "start", "name": "demo", "repo": "o/n"});
    for request in [&start, &env] {
        refused(
            &ask(&served.main, request),
            "log in first: moor user enroll",
        );
        let as_me = json!({"cmd": "as", "identity": me(), "request": request});
        refused(&ask(&served.main, &as_me), "log in first: moor user enroll");
        let as_other = json!({"cmd": "as", "identity": "local:4242", "request": request});
        refused(&ask(&served.main, &as_other), "speaks only for local:");
    }
    ask(&served.main, &json!({"cmd": "enroll"}));
    for request in [&start, &env] {
        refused(&ask(&served.main, request), "moor user secret set github");
    }
}

#[test]
fn start_needs_answers_for_the_identity_a_front_asserts() {
    let served = Served::start();
    let slack = served.front("slack");
    let ask_needs =
        |identity: &str| ask(&slack, &json!({"cmd": "start-needs", "identity": identity}));
    let got = ask_needs("slack:T1:U1");
    assert_eq!(got["class"], "ok", "{got}");
    assert_eq!(got["value"]["identity"], "slack:T1:U1", "{got}");
    assert_eq!(got["value"]["user"], json!(null), "{got}");
    assert_eq!(got["value"]["missing"], got["value"]["needs"], "{got}");
    assert_eq!(got["value"]["needs"][0], "claude", "{got}");
    refused(&ask_needs("web:alice"), "may assert only slack:");
}
