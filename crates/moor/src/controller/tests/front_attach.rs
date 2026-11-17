//! A person attaches to no one else's Box through a front socket.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

use moor_core::controller::PROTOCOL;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use serde_json::{json, Value};

use super::authz::not_yours;
use super::fronts::{ask, Served};
use super::host_paths::{ready_box, BOX};
use crate::store::Store;
use crate::testing::clock::FakeClock;

const ALICE: &str = "authgear:alice";
const BOB: &str = "authgear:bob";

/// A controller with the `authgear` front, alice's open Box, and bob enrolled.
fn served() -> Served {
    let dir = tempfile::tempdir().expect("tmp");
    let served = Served::start_fronts(dir, "fronts:\n  authgear: {}\n", &[]);
    let front = served.front("authgear");
    let enroll = |identity: &str, email: &str| {
        let req = json!({"cmd": "enroll", "identity": identity, "email": email});
        let got = ask(&front, &req);
        UserId::new(got["value"]["user"].as_i64().expect("user"))
    };
    let alice = enroll(ALICE, "alice@example.com");
    enroll(BOB, "bob@example.com");
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let db = served.dir.path().join("state").join("moor.db");
    ready_box(&Store::open(&db, clock).expect("store"), alice);
    served
}

/// Attaches to [`BOX`] on `socket` as `identity`: the relayed bytes and the response envelope.
fn attach(socket: &Path, identity: &str) -> (String, Value) {
    let mut write = UnixStream::connect(socket).expect("connect");
    let request = json!({"cmd": "as", "identity": identity,
        "request": {"cmd": "attach", "box": BOX, "cols": 80, "rows": 24}});
    let hello = json!({"frame": "hello", "protocol": PROTOCOL});
    let frame = json!({"frame": "request", "id": "1", "request": request});
    writeln!(write, "{hello}\n{frame}").expect("send");
    let mut read = BufReader::new(write);
    let (mut screen, mut line) = (Vec::new(), String::new());
    loop {
        line.clear();
        read.read_line(&mut line).expect("frame");
        let got: Value = serde_json::from_str(&line).expect("json frame");
        match got["frame"].as_str() {
            Some("bytes") => {
                let len = usize::try_from(got["len"].as_u64().expect("len")).expect("len");
                let mut raw = vec![0; len];
                read.read_exact(&mut raw).expect("bytes");
                screen.extend(raw);
            }
            Some("response") => {
                return (
                    String::from_utf8_lossy(&screen).into_owned(),
                    got["envelope"].clone(),
                )
            }
            _ => {}
        }
    }
}

#[test]
fn a_person_may_not_attach_to_someone_elses_box() {
    let served = served();
    let (screen, got) = attach(&served.front("authgear"), BOB);
    not_yours(&got);
    assert_eq!(screen, "");
}
