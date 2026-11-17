//! moor's own served host: a Box's plain request for `moor.moor.internal` reaches the gate's
//! Box listener stamped with the Box's id, never with one the Box sent, and only at a loopback
//! or private address.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::time::Duration;

use moor_core::config::Served;

use super::plain::{exchange, Plain};
use super::{addr, open_as, Server, World, PATH, PLAIN, TARGET};
use crate::proxy::egress::ServedRoute;

/// moor's host name, as Boxes request it.
const MOOR: &str = "moor.moor.internal";
/// The machine service's host name.
const MACHINE: &str = "machine.moor.internal";

/// A shared Proxy serving `routes`, each `(served, upstream)`.
fn server(w: &World, routes: &[(Served, String)]) -> Server {
    let mut cfg = w.config(10.0, 10.0, 64);
    cfg.served = routes
        .iter()
        .map(|(served, upstream)| ServedRoute {
            served: *served,
            host: served.default_host().to_owned(),
            upstream: upstream.clone(),
            key: (*served == Served::Machine).then(|| "machine-key".into()),
        })
        .collect();
    Server::start(cfg).unwrap()
}

/// `METHOD http://host/mcp` with `extra` headers, as a Box's agent sends it.
fn post(host: &str, extra: &str) -> String {
    format!(
        "POST http://{host}/mcp HTTP/1.1\r\nHost: {host}\r\n{extra}Content-Length: 2\r\n\r\n{{}}"
    )
}

#[test]
fn a_box_request_to_moor_reaches_the_upstream_with_its_own_box_id() {
    let w = World::new();
    let gate = Plain::start();
    let server = server(&w, &[(Served::Moor, gate.addr.to_string())]);
    let tunnel = open_as(&server, &w.token, &format!("{MOOR}:80"), true).unwrap();
    let reply = exchange(
        tunnel,
        &post(
            MOOR,
            "X-Moor-Box: 999\r\nx-moor-box: 998\r\nX-Moor-Machine: forged\r\n\
             X-Moor-Service-Key: forged\r\n",
        ),
    );
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    let heads = gate.heads();
    let [head] = heads.as_slice() else {
        unreachable!("{heads:?}")
    };
    assert!(head.starts_with("POST /mcp HTTP/1.1\r\n"), "{head}");
    let stamped: Vec<&str> = head
        .lines()
        .filter(|l| l.to_ascii_lowercase().starts_with("x-moor-"))
        .collect();
    assert_eq!(stamped, [format!("X-Moor-Box: {}", w.id.get())], "{head}");
    server.drain(Duration::from_secs(5));
}

/// The whole answer to a plain tunnel request for `target`, read as the forwarder would.
fn refusal(server: &Server, token: &str, target: &str) -> String {
    let mut stream = TcpStream::connect(addr(server)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let head = format!(
        "GET {PATH} HTTP/1.1\r\nHost: proxy\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Authorization: Bearer {token}\r\n{TARGET}: {target}\r\n{PLAIN}: 1\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).unwrap();
    let mut got = String::new();
    drop(stream.read_to_string(&mut got));
    got
}

#[test]
fn an_upstream_at_a_public_address_is_refused_and_never_dialled() {
    let w = World::new();
    for upstream in ["192.0.2.1:8444", "[2001:db8::1]:8444"] {
        let server = server(&w, &[(Served::Moor, upstream.to_owned())]);
        let got = refusal(&server, &w.token, &format!("{MOOR}:80"));
        assert!(got.starts_with("HTTP/1.1 502 "), "{upstream}: {got}");
        assert!(
            got.ends_with("the service's upstream is not a loopback or private address"),
            "{upstream}: {got}"
        );
        server.drain(Duration::from_secs(5));
    }
}

#[test]
fn without_an_entry_moor_is_not_served() {
    let w = World::new();
    let gate = Plain::start();
    let machine = Plain::start();
    let server = server(&w, &[(Served::Machine, machine.addr.to_string())]);
    if let Ok(tunnel) = open_as(&server, &w.token, &format!("{MOOR}:80"), true) {
        let reply = exchange(tunnel, &post(MOOR, ""));
        assert!(!reply.starts_with("HTTP/1.1 200"), "{reply}");
    }
    assert!(gate.heads().is_empty() && machine.heads().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn each_served_host_goes_to_its_own_upstream_with_its_own_headers() {
    let w = World::new();
    let gate = Plain::start();
    let machine = Plain::start();
    let server = server(
        &w,
        &[
            (Served::Machine, machine.addr.to_string()),
            (Served::Moor, gate.addr.to_string()),
        ],
    );
    for host in [MACHINE, MOOR] {
        let tunnel = open_as(&server, &w.token, &format!("{host}:80"), true).unwrap();
        let reply = exchange(tunnel, &post(host, ""));
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "{host}: {reply}");
    }
    let (machine, gate) = (machine.heads(), gate.heads());
    let ([machine], [gate]) = (machine.as_slice(), gate.as_slice()) else {
        unreachable!("{machine:?} {gate:?}")
    };
    assert!(machine.contains("X-Moor-Machine-Missing: "), "{machine}");
    assert!(
        machine.contains("X-Moor-Service-Key: machine-key\r\n"),
        "{machine}"
    );
    assert!(!gate.contains("X-Moor-Machine"), "{gate}");
    assert!(!gate.contains("X-Moor-Service-Key"), "{gate}");
    server.drain(Duration::from_secs(5));
}
