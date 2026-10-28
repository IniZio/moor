//! A person attaches to the terminal of their own Box through a front socket, and to no one
//! else's.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use moor_core::controller::PROTOCOL;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use serde_json::{json, Value};

use super::authz::not_yours;
use super::fronts::{ask, Served};
use super::host_paths::{ready_box, BOX, FAKE_CRABBOX};
use crate::store::Store;
use crate::testing::clock::FakeClock;

const ALICE: &str = "authgear:alice";
const BOB: &str = "authgear:bob";

/// An ssh that answers `ssh -G` as pinned words do and otherwise prints [`SCREEN`], as the
/// Box's terminal would.
const FAKE_SSH: &str = "#!/bin/sh\n\
    test \"$1\" = -G && { echo 'stricthostkeychecking true'; echo 'userknownhostsfile /k'; exit; }\n\
    echo moor-fake-terminal\n";

/// What [`FAKE_SSH`] prints.
const SCREEN: &str = "moor-fake-terminal";

/// The directory of [`FAKE_SSH`], first on this process's `PATH`: the relay starts a bare `ssh`
/// in the controller's own process, so no variable of the controller's reaches it.
fn fake_ssh_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tmp").keep();
        moor_testkit::write_executable(&dir.join("ssh"), FAKE_SSH).expect("fake ssh");
        let old = std::env::var_os("PATH").unwrap_or_default();
        let path =
            std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(&old)))
                .expect("path");
        std::env::set_var("PATH", path);
        dir
    })
}

/// A controller with the `authgear` front, alice's open Box, and bob enrolled.
fn served() -> Served {
    let dir = tempfile::tempdir().expect("tmp");
    let crabbox = dir.path().join("crabbox");
    moor_testkit::write_executable(&crabbox, FAKE_CRABBOX).expect("fake crabbox");
    let ssh = fake_ssh_dir().join("ssh");
    let served = Served::start_fronts(
        dir,
        "fronts:\n  authgear: {}\n",
        &[
            ("MOOR_CRABBOX_BIN", crabbox.to_str().expect("utf8")),
            ("MOOR_SSH_BIN", ssh.to_str().expect("utf8")),
        ],
    );
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
fn a_person_attaches_to_their_own_box_through_a_front_socket() {
    let served = served();
    let (screen, got) = attach(&served.front("authgear"), ALICE);
    assert_eq!(got["class"], "ok", "{got}");
    assert!(screen.contains(SCREEN), "relayed {screen:?}");
    assert_eq!(got["value"]["bytes_out"], screen.len(), "{got}");
}

#[test]
fn a_person_may_not_attach_to_someone_elses_box() {
    let served = served();
    let (screen, got) = attach(&served.front("authgear"), BOB);
    not_yours(&got);
    assert_eq!(screen, "");
}
