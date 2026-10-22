//! A `run` whose client is killed mid-run is cancelled and recorded interrupted.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::controller::{ClientFrame, PROTOCOL};
use moor_core::user::UserId;
use moor_core::{BoxId, BoxName, Request, RunReq};
use serde_json::json;

use super::fronts::{ask, Served};
use super::host_paths::{ready_box, BOX};
use crate::adapters::clock::SystemClock;
use crate::ops::run_fixture::req;
use crate::proxy::supervisor::LEASE;
use crate::store::{Fenced, Lease, ReadyProc, Reserve, Store};
use crate::testing::rng::FakeRng;

/// Polls of [`POLL`] a test waits before it gives up, 10 s in all.
const POLLS: u32 = 500;

/// Pause between two polls.
const POLL: Duration = Duration::from_millis(20);

/// A crabbox whose `run` marks `started` beside itself and then outlasts the test.
const FAKE_CRABBOX: &str = "#!/bin/sh\n\
    PATH=/usr/bin:/bin\n\
    case \"$1\" in\n\
    --version) echo 'crabbox 0.79.1-moor.1' ;;\n\
    run) touch \"$(dirname \"$0\")/started\"; exec sleep 60 ;;\n\
    *) exit 1 ;;\n\
    esac\n";

/// A client process holding the only end of a connection that asked for `request`.
fn client_process(socket: &Path, request: &Request) -> std::process::Child {
    let mut stream = UnixStream::connect(socket).unwrap();
    let hello = ClientFrame::Hello { protocol: PROTOCOL };
    stream.write_all(hello.to_line().as_bytes()).unwrap();
    let mut line = String::new();
    BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    let ask = ClientFrame::Request {
        id: "1".to_owned(),
        request: Box::new(request.clone()),
    };
    stream.write_all(ask.to_line().as_bytes()).unwrap();
    let mut holder = Command::new("sleep");
    holder.arg("30").stdin(Stdio::from(OwnedFd::from(stream)));
    holder.spawn().unwrap()
}

/// Records a ready Proxy for Box `id` whose supervisor holds a fresh lease, so a run reuses it.
fn proxy_ready(store: &Store, id: BoxId) {
    let reserved = store.reserve(id, &FakeRng::default(), LEASE).unwrap();
    assert!(matches!(reserved, Reserve::Reserved(_)), "{reserved:?}");
    assert_eq!(
        store.claim_lease(id, "test", LEASE).unwrap(),
        Lease::Claimed
    );
    let proc = ReadyProc {
        pid: std::process::id(),
        handle: "h",
        boot_id: None,
    };
    let marked = store.mark_ready(id, "test", 2000, proc).unwrap();
    assert_eq!(marked, Fenced::Applied);
}

/// Polls `done` until it holds or [`POLLS`] polls pass.
fn within_wait(mut done: impl FnMut() -> bool) -> bool {
    (0..POLLS).any(|_| {
        done() || {
            thread::sleep(POLL);
            false
        }
    })
}

#[test]
fn a_run_whose_client_is_killed_is_cancelled_and_recorded_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let crabbox = dir.path().join("crabbox");
    moor_testkit::write_executable(&crabbox, FAKE_CRABBOX).unwrap();
    let served = Served::start_in(dir, &[("MOOR_CRABBOX_BIN", crabbox.to_str().unwrap())]);
    let db = served.dir.path().join("state").join("moor.db");
    let store = Store::open(&db, Arc::new(SystemClock::real())).unwrap();
    let operator = ask(&served.main, &json!({"cmd": "enroll"}))["value"]["user"]
        .as_i64()
        .unwrap();
    let id = ready_box(&store, UserId::new(operator));
    let run = RunReq {
        box_name: BoxName::parse_allow_bare(BOX).unwrap(),
        ..req(&["sleep", "600"])
    };
    proxy_ready(&store, id);
    let mut client = client_process(&served.main, &Request::Run(run));
    let started = served.dir.path().join("started");
    assert!(within_wait(|| started.exists()), "the command started");
    client.kill().unwrap();
    client.wait().unwrap();
    let interrupted = within_wait(|| {
        store
            .last_run(id)
            .unwrap()
            .is_some_and(|run| run.interrupted)
    });
    assert!(
        interrupted,
        "the run was cancelled and recorded interrupted"
    );
}
