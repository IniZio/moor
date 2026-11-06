//! A replica learns every open Box from the ledger, turns Ready only after it has, and serves
//! any of them while the controller is away.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::time::Duration;

use super::{addr, eventually, open, Server, World};

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
