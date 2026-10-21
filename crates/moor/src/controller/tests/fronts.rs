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

use super::{config, env, Buf};
use crate::controller::{start, Sink};

/// A running controller with a `slack` front, and where its sockets and stderr are.
pub(super) struct Served {
    /// Holds the config, the vault key and the state dir.
    pub(super) dir: tempfile::TempDir,
    /// The controller socket.
    pub(super) main: PathBuf,
}

impl Served {
    /// Starts a controller with the `slack` and `web` fronts.
    pub(super) fn start() -> Self {
        let dir = tempfile::tempdir().expect("tmp");
        let cfg = config(dir.path(), "fronts:\n  slack: {}\n  web: {}\n");
        let running = start(&env(dir.path()), &cfg).expect("start");
        let main = running.socket.clone();
        let sink: Sink = Arc::new(Mutex::new(Buf::default()));
        std::thread::spawn(move || running.accept_loop(&sink));
        Self { dir, main }
    }

    /// The socket of front `ns`.
    pub(super) fn front(&self, ns: &str) -> PathBuf {
        self.dir
            .path()
            .join("state")
            .join("fronts")
            .join(format!("{ns}.sock"))
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
