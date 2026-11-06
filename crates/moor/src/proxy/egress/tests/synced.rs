//! A replica learns every awake Box from the ledger, turns Ready only after a whole pass in
//! time, and serves any of them while the controller is away or slow.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::BoxName;

use super::{addr, eventually, open, Server, Warm, World, PATH, TARGET};
use crate::proxy::ledger::tests::EGRESS_KEY;
use crate::proxy::token;
use crate::testing::rng::FakeRng;

/// The status line `server` answers `GET /synced` with.
pub(super) fn synced(server: &Server) -> String {
    let mut probe = TcpStream::connect(addr(server)).unwrap();
    probe
        .write_all(b"GET /synced HTTP/1.1\r\nHost: egress\r\n\r\n")
        .unwrap();
    let mut answer = String::new();
    probe.read_to_string(&mut answer).unwrap();
    answer.lines().next().unwrap_or_default().to_owned()
}

#[test]
fn a_replica_is_synced_once_the_ledger_answers_and_stays_synced_without_it() {
    let mut w = World::new();
    let server = w.server(10.0);
    eventually("the first sync", || synced(&server) == "HTTP/1.1 200 OK");
    drop(w.ledger.take());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(synced(&server), "HTTP/1.1 200 OK");
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_replica_that_never_reached_the_ledger_is_not_synced() {
    let mut w = World::new();
    drop(w.ledger.take());
    let server = w.server(10.0);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(synced(&server), "HTTP/1.1 503 Service Unavailable");
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_replica_serves_a_box_it_never_saw_while_the_controller_is_away() {
    let mut w = World::new();
    let server = w.server(10.0);
    eventually("the first sync", || synced(&server) == "HTTP/1.1 200 OK");
    drop(w.ledger.take());
    let tunnel = open(&server, &w.token, &w.target()).unwrap();
    assert_eq!(w.get(tunnel), "HTTP/1.1 200 OK");
    let seen = w.up.seen();
    assert_eq!(
        seen.last().and_then(|s| s.authorization.as_deref()),
        Some("Bearer test-token-1")
    );
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_replica_with_an_open_box_it_cannot_ready_is_not_synced() {
    let w = World::new();
    let bare = w
        .store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("bare").unwrap(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    drop(w.store.reserve(bare, &FakeRng::default()));
    token::mint(&w.store, EGRESS_KEY.as_bytes(), bare)
        .unwrap()
        .unwrap();
    let server = w.server(10.0);
    eventually("a failed pass", || {
        String::from_utf8_lossy(&w.log.lock().unwrap()).contains("no proxy spec")
    });
    assert_eq!(synced(&server), "HTTP/1.1 503 Service Unavailable");
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_pass_slower_than_its_deadline_does_not_count() {
    let w = World::new();
    let mut cfg = w.config(10.0, 10.0, 64);
    cfg.warm = Some(Warm {
        every: Duration::from_secs(10),
        deadline: Duration::ZERO,
    });
    let server = Server::start(cfg).unwrap();
    eventually("a pass to finish", || server.serving() == 1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(synced(&server), "HTTP/1.1 503 Service Unavailable");
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_known_token_waits_about_a_second_on_a_ledger_that_never_answers() {
    let mut w = World::new();
    let server = w.server(10.0);
    eventually("the first sync", || synced(&server) == "HTTP/1.1 200 OK");
    drop(w.ledger.take());
    let socket = w.dir.path().join("proxy.sock");
    let silent = UnixListener::bind(&socket).unwrap();
    silent.set_nonblocking(true).unwrap();
    let held = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&held);
    std::thread::spawn(move || {
        while Arc::strong_count(&keep) > 1 {
            if let Ok((stream, _)) = silent.accept() {
                keep.lock().unwrap().push(stream);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let mut raw = TcpStream::connect(addr(&server)).unwrap();
    raw.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
    write!(
        raw,
        "GET {PATH} HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Authorization: Bearer {}\r\n{TARGET}: {}\r\n\r\n",
        w.token,
        w.target()
    )
    .unwrap();
    let mut status = [0_u8; 12];
    raw.read_exact(&mut status).unwrap();
    assert_eq!(&status, b"HTTP/1.1 101");
    drop(raw);
    for stream in held.lock().unwrap().drain(..) {
        drop(stream.shutdown(std::net::Shutdown::Both));
    }
    drop(held);
    std::fs::remove_file(&socket).unwrap();
    server.drain(Duration::from_secs(5));
}
