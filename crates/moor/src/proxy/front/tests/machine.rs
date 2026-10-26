use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use moor_core::config::MachineService;
use moor_core::{BoxId, Secret};
use tempfile::TempDir;

use super::{dial, fake, read_all, socket_in, uid, Answer, Fake, INTERNAL, WAIT};
use crate::proxy::front::{Front, Held, MachineRoute};

const HOST: &str = "machine.moor.internal";

/// The service on loopback: sends each request head it reads, then answers `200 ok`.
fn service() -> (SocketAddr, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { return };
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") && conn.read(&mut byte).unwrap_or(0) == 1 {
                head.extend_from_slice(&byte);
            }
            tx.send(String::from_utf8_lossy(&head).into_owned())
                .unwrap();
            let _ = conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        }
    });
    (addr, rx)
}

/// A Box 7 front with the machine service; the Box holds `mac` while `holds` is set.
struct Rig {
    _dir: TempDir,
    front: Front,
    nono: Fake,
    upstream: Receiver<String>,
    holds: Arc<AtomicBool>,
}

fn rig() -> Rig {
    let nono = fake(Answer::Echo);
    let (upstream_addr, upstream) = service();
    let holds = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&holds);
    let route = MachineRoute {
        service: MachineService {
            host: HOST.to_owned(),
            upstream: upstream_addr,
        },
        held: Arc::new(move || {
            flag.load(Ordering::Acquire).then(|| Held {
                box_id: BoxId::new(7),
                name: "mac".to_owned(),
                ssh: "dev@mac.example.com:22".to_owned(),
                slot: 1,
                sync_exclude: vec!["node_modules".to_owned(), "app/build".to_owned()],
            })
        }),
    };
    let dir = tempfile::tempdir().unwrap();
    let front = Front::start(
        nono.addr,
        socket_in(&dir, uid()),
        Secret::new(INTERNAL.as_bytes().to_vec()),
        Some(route),
    )
    .unwrap();
    Rig {
        _dir: dir,
        front,
        nono,
        upstream,
        holds,
    }
}

impl Rig {
    fn send(&self, request: &str) -> String {
        let mut c = dial(&self.front);
        c.write_all(request.as_bytes()).unwrap();
        read_all(&mut c)
    }

    fn quiet(&self) {
        assert!(
            self.upstream.recv_timeout(WAIT / 10).is_err(),
            "service saw a request"
        );
        assert!(
            self.nono.seen.recv_timeout(WAIT / 10).is_err(),
            "nono saw a request"
        );
    }
}

#[test]
fn a_plain_request_to_the_service_host_reaches_it_with_the_box_and_machine() {
    let rig = rig();
    let reply = rig.send(&format!(
        "GET http://{HOST}/mcp HTTP/1.1\r\nHost: {HOST}\r\n\r\n"
    ));
    assert!(
        reply.starts_with("HTTP/1.1 200 OK") && reply.ends_with("ok"),
        "{reply}"
    );
    let head = rig.upstream.recv_timeout(WAIT).unwrap();
    assert!(head.starts_with("GET /mcp HTTP/1.1\r\n"), "{head}");
    for line in [
        "X-Moor-Box: 7\r\n",
        "X-Moor-Machine: mac\r\n",
        "X-Moor-Machine-Ssh: dev@mac.example.com:22\r\n",
        "X-Moor-Machine-Slot: 1\r\n",
        "X-Moor-Machine-Exclude: [\"node_modules\",\"app/build\"]\r\n",
    ] {
        assert!(head.contains(line), "{line} missing from {head}");
    }
    rig.quiet();
}

#[test]
fn identity_headers_the_box_sends_are_replaced() {
    let rig = rig();
    let reply = rig.send(&format!(
        "POST http://{HOST}:8080/mcp HTTP/1.1\r\nHost: {HOST}\r\nX-Moor-Machine: other\r\nx-moor-box: 99\r\nX-Moor-Machine-Exclude: []\r\nX-MOOR-Extra: 1\r\nContent-Length: 0\r\n\r\n"
    ));
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    let head = rig.upstream.recv_timeout(WAIT).unwrap();
    let own: Vec<&str> = head
        .split("\r\n")
        .filter(|l| l.to_ascii_lowercase().starts_with("x-moor-"))
        .collect();
    assert_eq!(
        own,
        [
            "X-Moor-Box: 7",
            "X-Moor-Machine: mac",
            "X-Moor-Machine-Ssh: dev@mac.example.com:22",
            "X-Moor-Machine-Slot: 1",
            "X-Moor-Machine-Exclude: [\"node_modules\",\"app/build\"]"
        ]
    );
}

#[test]
fn a_box_without_a_machine_is_refused_and_the_hold_is_read_per_request() {
    let rig = rig();
    rig.holds.store(false, Ordering::Release);
    let request = format!("GET http://{HOST}/mcp HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
    assert!(rig.send(&request).starts_with("HTTP/1.1 403"));
    rig.quiet();
    rig.holds.store(true, Ordering::Release);
    assert!(rig.send(&request).starts_with("HTTP/1.1 200 OK"));
}

#[test]
fn a_connect_to_the_service_host_is_refused() {
    let rig = rig();
    for authority in [format!("{HOST}:443"), "MACHINE.moor.internal:80".to_owned()] {
        let reply = rig.send(&format!(
            "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n"
        ));
        assert!(reply.starts_with("HTTP/1.1 403"), "{authority}: {reply}");
    }
    rig.quiet();
}

#[test]
fn other_hosts_still_go_to_nono() {
    let rig = rig();
    let mut c = dial(&rig.front);
    c.write_all(b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\nX-Moor-Box: 1\r\n\r\n")
        .unwrap();
    let head = String::from_utf8(rig.nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert!(
        head.starts_with("CONNECT example.com:80 HTTP/1.1\r\n"),
        "{head}"
    );
    assert!(
        rig.upstream.recv_timeout(WAIT / 10).is_err(),
        "service saw a request"
    );
    let rig = self::rig();
    let mut c = dial(&rig.front);
    c.write_all(b"CONNECT machine.moor.internal.example.com:443 HTTP/1.1\r\n\r\n")
        .unwrap();
    let head = String::from_utf8(rig.nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert!(
        head.starts_with("CONNECT machine.moor.internal.example.com:443 "),
        "{head}"
    );
    assert!(
        rig.upstream.recv_timeout(WAIT / 10).is_err(),
        "service saw a request"
    );
}
