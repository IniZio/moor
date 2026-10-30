use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use moor_core::config::{MachineKind, MachineService};
use moor_core::{BoxId, Secret};
use tempfile::TempDir;

use super::{dial, fake, read_all, socket_in, uid, Answer, Fake, INTERNAL, WAIT};
use crate::proxy::front::{Front, Held, Hold, MachineRoute};

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

const NO_CLAIM: &str = "this Box has no machine: its repo's .moor.yaml asks for none, or none was free when it started";

/// A Box 7 front with the machine service; the Box holds `mac` while `holds` is set, and
/// otherwise lacks a slot for the reason in `missing`.
struct Rig {
    _dir: TempDir,
    front: Front,
    nono: Fake,
    upstream: Receiver<String>,
    holds: Arc<AtomicBool>,
    missing: Arc<Mutex<String>>,
    kind: Arc<Mutex<MachineKind>>,
}

fn rig() -> Rig {
    let nono = fake(Answer::Echo);
    let (upstream_addr, upstream) = service();
    let holds = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&holds);
    let missing = Arc::new(Mutex::new(NO_CLAIM.to_owned()));
    let why = Arc::clone(&missing);
    let kind = Arc::new(Mutex::new(MachineKind::Physical));
    let held_kind = Arc::clone(&kind);
    let route = MachineRoute {
        service: MachineService {
            host: HOST.to_owned(),
            upstream: upstream_addr,
        },
        held: Arc::new(move || {
            if flag.load(Ordering::Acquire) {
                Hold::Held(Held {
                    box_id: BoxId::new(7),
                    name: "mac".to_owned(),
                    ssh: "dev@mac.example.com:22".to_owned(),
                    machine_id: None,
                    slot: 1,
                    kind: held_kind.lock().unwrap().clone(),
                    ports: vec![8081, 8090],
                    sync_exclude: vec!["node_modules".to_owned(), "app/build".to_owned()],
                })
            } else {
                Hold::Missing {
                    box_id: BoxId::new(7),
                    reason: why.lock().unwrap().clone(),
                }
            }
        }),
    };
    let dir = tempfile::tempdir().unwrap();
    let front = Front::start(
        nono.addr,
        socket_in(&dir, uid()),
        Secret::new(INTERNAL.as_bytes().to_vec()),
        Some(route),
        std::sync::Arc::new(std::sync::Mutex::new(std::io::sink())),
    )
    .unwrap();
    Rig {
        _dir: dir,
        front,
        nono,
        upstream,
        holds,
        missing,
        kind,
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
            "X-Moor-Machine-Exclude: [\"node_modules\",\"app/build\"]",
            "X-Moor-Machine-Kind: physical",
            "X-Moor-Machine-Ports: 8081,8090"
        ]
    );
}

fn own_headers(head: &str) -> Vec<&str> {
    head.split("\r\n")
        .filter(|l| l.to_ascii_lowercase().starts_with("x-moor-machine-"))
        .collect()
}

#[test]
fn a_vm_host_slot_is_marked_with_its_kind_provider_and_ports() {
    let rig = rig();
    *rig.kind.lock().unwrap() = MachineKind::VmHost {
        provider: "external".to_owned(),
    };
    rig.send(&format!(
        "GET http://{HOST}/mcp HTTP/1.1\r\nHost: {HOST}\r\n\r\n"
    ));
    let head = rig.upstream.recv_timeout(WAIT).unwrap();
    let own = own_headers(&head);
    for line in [
        "X-Moor-Machine-Kind: vm-host",
        "X-Moor-Machine-Vm: external",
        "X-Moor-Machine-Ports: 8081,8090",
    ] {
        assert!(own.contains(&line), "{line} missing from {own:?}");
    }
}

#[test]
fn a_physical_machine_has_no_vm_header() {
    let rig = rig();
    rig.send(&format!(
        "GET http://{HOST}/mcp HTTP/1.1\r\nHost: {HOST}\r\n\r\n"
    ));
    let head = rig.upstream.recv_timeout(WAIT).unwrap();
    let own = own_headers(&head);
    assert!(own.contains(&"X-Moor-Machine-Kind: physical"), "{own:?}");
    assert!(
        !own.iter().any(|l| l.starts_with("X-Moor-Machine-Vm")),
        "{own:?}"
    );
}

#[test]
fn kind_vm_and_ports_headers_the_box_sends_are_dropped() {
    let rig = rig();
    rig.send(&format!(
        "GET http://{HOST}/mcp HTTP/1.1\r\nHost: {HOST}\r\nX-Moor-Machine-Kind: vm-host\r\nx-moor-machine-vm: forged\r\nX-MOOR-MACHINE-PORTS: 1\r\n\r\n"
    ));
    let head = rig.upstream.recv_timeout(WAIT).unwrap();
    let own = own_headers(&head);
    assert!(own.contains(&"X-Moor-Machine-Kind: physical"), "{own:?}");
    assert!(own.contains(&"X-Moor-Machine-Ports: 8081,8090"), "{own:?}");
    assert!(!head.to_ascii_lowercase().contains("forged"), "{head}");
    assert!(!head.contains("vm-host"), "{head}");
}

#[test]
fn a_box_without_a_slot_reaches_the_service_marked_with_why_and_is_read_per_request() {
    let rig = rig();
    rig.holds.store(false, Ordering::Release);
    let request = format!(
        "GET http://{HOST}/mcp HTTP/1.1\r\nHost: {HOST}\r\nX-Moor-Machine-Missing: forged\r\nX-Moor-Machine: other\r\n\r\n"
    );
    assert!(rig.send(&request).starts_with("HTTP/1.1 200 OK"));
    let head = rig.upstream.recv_timeout(WAIT).unwrap();
    let own: Vec<&str> = head
        .split("\r\n")
        .filter(|l| l.to_ascii_lowercase().starts_with("x-moor-"))
        .collect();
    assert_eq!(
        own,
        [
            "X-Moor-Box: 7".to_owned(),
            format!("X-Moor-Machine-Missing: {NO_CLAIM}")
        ]
    );
    *rig.missing.lock().unwrap() = "machine mac is released while this Box sleeps".to_owned();
    assert!(rig.send(&request).starts_with("HTTP/1.1 200 OK"));
    let head = rig.upstream.recv_timeout(WAIT).unwrap();
    assert!(
        head.contains("X-Moor-Machine-Missing: machine mac is released while this Box sleeps\r\n"),
        "{head}"
    );
    rig.holds.store(true, Ordering::Release);
    assert!(rig.send(&request).starts_with("HTTP/1.1 200 OK"));
    let head = rig.upstream.recv_timeout(WAIT).unwrap();
    assert!(
        head.contains("X-Moor-Machine: mac\r\n") && !head.contains("Missing"),
        "{head}"
    );
}

#[test]
fn a_reason_with_control_characters_is_refused() {
    let rig = rig();
    rig.holds.store(false, Ordering::Release);
    *rig.missing.lock().unwrap() = "bad\r\nX-Evil: 1".to_owned();
    let request = format!("GET http://{HOST}/mcp HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
    assert!(rig.send(&request).starts_with("HTTP/1.1 403"));
    rig.quiet();
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
