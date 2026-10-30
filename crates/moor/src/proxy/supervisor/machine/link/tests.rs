#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use moor_client::forward::{Forward, Forwarding};
use moor_core::cache::CachePlan;
use moor_core::name::BoxName;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::{BoxHandle, BoxId, ProviderId};

use super::{keep, MachineLink, Via};
use crate::testing::driver::FakeDriver;

fn boxed(fake: &FakeDriver) -> BoxHandle {
    fake.acquire(&AcquireReq {
        pool_machine: None,
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").expect("name"),
        class: None,
        nonce: "n0".to_owned(),
        cache: CachePlan::none(),
    })
    .expect("acquired")
    .handle
}

/// A runner that answers each forward request with `answer`, holds the connection, and sends
/// every request it read plus its held connection back.
fn runner(socket: &Path, answer: Forwarding) -> mpsc::Receiver<(Forward, UnixStream)> {
    let listener = UnixListener::bind(socket).expect("bind");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut conn = conn.expect("accept");
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).expect("read");
            let req: Forward = serde_json::from_str(&line).expect("forward request");
            let mut out = serde_json::to_vec(&answer).expect("answer");
            out.push(b'\n');
            conn.write_all(&out).expect("write");
            if tx.send((req, conn)).is_err() {
                return;
            }
        }
    });
    rx
}

fn via(socket: &Path) -> Via<'_> {
    Via {
        socket,
        box_id: 7,
        machine_id: 3,
    }
}

#[test]
fn opens_a_tunnel_per_port_then_asks_the_runner_to_serve_them() {
    let dir = tempfile::tempdir().expect("dir");
    let socket = dir.path().join("forward.sock");
    let seen = runner(&socket, Forwarding::Up);
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let mut link = MachineLink::open(&driver, &h, &via(&socket), &[8081, 3001]).expect("open");
    assert!(link.alive());
    let ports: Vec<u16> = driver.tunnels().iter().map(|t| t.box_port).collect();
    assert_eq!(ports, [8081, 3001]);
    let (req, _conn) = seen.recv().expect("runner asked");
    assert_eq!((req.box_id, req.machine_id), (7, 3));
    assert_eq!(req.ports, [(8081, 28081), (3001, 23001)]);
}

#[test]
fn a_dead_link_is_reopened_and_close_ends_every_tunnel() {
    let dir = tempfile::tempdir().expect("dir");
    let socket = dir.path().join("forward.sock");
    let seen = runner(&socket, Forwarding::Up);
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let open = || MachineLink::open(&driver, &h, &via(&socket), &[8081]);
    let mut link = Some(open().expect("open"));
    let first = driver.tunnels().len();
    keep(&mut link, open).expect("alive link kept");
    assert_eq!(driver.tunnels().len(), first, "a live link is left alone");
    let (_, held) = seen.recv().expect("runner asked");
    drop(held);
    keep(&mut link, open).expect("reopened");
    assert_eq!(
        driver.tunnels().len(),
        first + 1,
        "the link the runner let go was opened again"
    );
    drop(link);
    assert!(
        driver.dropped_tunnels().contains(&50_001),
        "close ends the tunnel"
    );
}

#[test]
fn a_failed_tunnel_names_its_port() {
    let dir = tempfile::tempdir().expect("dir");
    let socket = dir.path().join("forward.sock");
    let driver = FakeDriver::new();
    let h = boxed(&driver);
    let err = MachineLink::open(&driver, &h, &via(&socket), &[8081])
        .err()
        .expect("no tunnel support");
    assert_eq!(err.port, Some(8081));
}

#[test]
fn a_refused_forward_carries_the_runner_reason() {
    let dir = tempfile::tempdir().expect("dir");
    let socket = dir.path().join("forward.sock");
    let _seen = runner(
        &socket,
        Forwarding::Refused {
            reason: "mini is not linked".to_owned(),
        },
    );
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let err = MachineLink::open(&driver, &h, &via(&socket), &[8081])
        .err()
        .expect("refused");
    assert_eq!((err.port, err.error.as_str()), (None, "mini is not linked"));
}
