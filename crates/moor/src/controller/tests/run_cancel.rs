//! A `run` is cancelled only when its client leaves: a killed client cancels it, a drain does not.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
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

/// A crabbox that reports a supported version and the ssh target of every Box.
const FAKE_CRABBOX: &str = "#!/bin/sh\n\
    case \"$1\" in\n\
    --version) echo 'crabbox 0.79.1-moor.1' ;;\n\
    ssh) echo 'ssh -o StrictHostKeyChecking=yes u@box' ;;\n\
    *) exit 1 ;;\n\
    esac\n";

/// An ssh that answers `ssh -G` as crabbox's pinned words do; a Box command marks `started`
/// beside itself and then outlasts the test, and any other remote command runs on this host.
const FAKE_SSH: &str = "#!/bin/sh\n\
    PATH=/usr/bin:/bin\n\
    test \"$1\" = -G && { echo 'stricthostkeychecking true'; echo 'userknownhostsfile /k'; exit; }\n\
    case \"$*\" in *gate-*) touch \"$(dirname \"$0\")/started\"; exec sleep 60 ;; esac\n\
    for last; do :; done\nexec sh -c \"$last\"\n";

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
    let ssh = dir.path().join("ssh");
    moor_testkit::write_executable(&ssh, FAKE_SSH).unwrap();
    let served = Served::start_in(
        dir,
        &[
            ("MOOR_CRABBOX_BIN", crabbox.to_str().unwrap()),
            ("MOOR_SSH_BIN", ssh.to_str().unwrap()),
        ],
    );
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

/// [`FAKE_SSH`] whose Box command marks `started` and outlasts the start of a drain before it
/// runs on this host, with a home beside itself for the Proxy CA.
const SLOW_SSH: &str = "#!/bin/sh\n\
    PATH=\"$(dirname \"$0\")/bin:/usr/bin:/bin\"\n\
    export HOME=\"$(dirname \"$0\")\"\n\
    test \"$1\" = -G && { echo 'stricthostkeychecking true'; echo 'userknownhostsfile /k'; exit; }\n\
    case \"$*\" in *gate-*) touch \"$(dirname \"$0\")/started\"; sleep 1 ;; esac\n\
    for last; do :; done\nexec sh -c \"$last\"\n";

/// A connection to `socket` that has said hello.
fn greeted(socket: &Path) -> (UnixStream, BufReader<UnixStream>) {
    let mut stream = UnixStream::connect(socket).unwrap();
    let hello = ClientFrame::Hello { protocol: PROTOCOL };
    stream.write_all(hello.to_line().as_bytes()).unwrap();
    let mut read = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    read.read_line(&mut line).unwrap();
    (stream, read)
}

/// The next frame `read` gets.
fn frame(read: &mut BufReader<UnixStream>) -> serde_json::Value {
    let mut line = String::new();
    read.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// The response or error frame that ends a request on `read`, past its progress and output.
fn reply(read: &mut BufReader<UnixStream>) -> serde_json::Value {
    loop {
        let next = frame(read);
        if next["frame"] == "response" || next["frame"] == "error" {
            return next;
        }
        if let Some(len) = next["len"].as_u64() {
            let mut payload = vec![0; usize::try_from(len).unwrap()];
            read.read_exact(&mut payload).unwrap();
        }
    }
}

#[test]
fn a_drain_lets_a_run_in_flight_finish_and_closes_idle_connections() {
    let dir = tempfile::tempdir().unwrap();
    let (crabbox, ssh) = (dir.path().join("crabbox"), dir.path().join("ssh"));
    moor_testkit::write_executable(&crabbox, FAKE_CRABBOX).unwrap();
    moor_testkit::write_executable(&ssh, SLOW_SSH).unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let _egress =
        moor_testkit::FakeEgress::start_on(dir.path(), &bin, crate::store::egress_port(2000))
            .unwrap();
    let env = super::env_with(
        dir.path(),
        &[
            ("MOOR_CRABBOX_BIN", crabbox.to_str().unwrap()),
            ("MOOR_SSH_BIN", ssh.to_str().unwrap()),
        ],
    );
    let cfg = super::config(dir.path(), "drain_timeout_secs: 10\n");
    let running = crate::controller::start(&env, &cfg).unwrap();
    let socket = running.socket.clone();
    let log = super::Buf::default();
    let (mut signal, serving) = running.serve_until_signalled(Arc::new(Mutex::new(log.clone())));
    let store = Store::open(
        &dir.path().join("state").join("moor.db"),
        Arc::new(SystemClock::real()),
    )
    .unwrap();
    let operator = ask(&socket, &json!({"cmd": "enroll"}))["value"]["user"]
        .as_i64()
        .unwrap();
    let id = ready_box(&store, UserId::new(operator));
    proxy_ready(&store, id);
    let (_idle, mut idle_read) = greeted(&socket);
    let (mut busy, mut busy_read) = greeted(&socket);
    let run = RunReq {
        box_name: BoxName::parse_allow_bare(BOX).unwrap(),
        ..req(&["sh", "-c", "exit 7"])
    };
    let ask_run = ClientFrame::Request {
        id: "r".to_owned(),
        request: Box::new(Request::Run(run)),
    };
    busy.write_all(ask_run.to_line().as_bytes()).unwrap();
    let started = dir.path().join("started");
    assert!(within_wait(|| started.exists()), "the command started");
    signal.write_all(b"x").unwrap();
    let ended = frame(&mut idle_read);
    assert_eq!(ended["kind"], "restarting", "{ended}");
    let mut rest = String::new();
    idle_read.read_line(&mut rest).unwrap();
    assert_eq!(rest, "", "the idle connection was closed by the drain");
    let reply = reply(&mut busy_read);
    assert_eq!(reply["frame"], "response", "{reply}");
    let last = store.last_run(id).unwrap().unwrap();
    assert_eq!(
        (last.exit, last.interrupted),
        (Some(7), false),
        "the run ended with its own exit: {reply}"
    );
    let (tx, exited) = std::sync::mpsc::channel();
    thread::spawn(move || tx.send(serving.join().is_ok()).ok());
    let exited = exited.recv_timeout(Duration::from_secs(10));
    assert_eq!(exited, Ok(true), "the controller exited after its drain");
    assert!(log.text().contains("drained; exiting"), "{}", log.text());
}
