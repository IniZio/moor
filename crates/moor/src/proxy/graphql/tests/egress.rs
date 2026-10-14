//! Destinations other than the judged host: refused unless the guard allows them.

use std::net::{Ipv4Addr, TcpListener};

use super::*;
use crate::proxy::graphql::{EGRESS_RULE, GUARD_CATEGORY};

/// Loopback listener that records how many connections it accepted.
struct Probe {
    /// Where it listens.
    addr: SocketAddr,
    /// Connections accepted so far.
    accepted: Arc<AtomicUsize>,
}

/// Starts a listener; with `echo` it sends back every byte it receives.
fn probe(echo: bool) -> Probe {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&accepted);
    std::thread::spawn(move || {
        for _ in 0..2000 {
            if let Ok((mut tcp, _)) = listener.accept() {
                count.fetch_add(1, Ordering::SeqCst);
                if echo {
                    std::thread::spawn(move || {
                        tcp.set_nonblocking(false).unwrap();
                        tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                        let mut buf = [0u8; 256];
                        while let Ok(n @ 1..) = tcp.read(&mut buf) {
                            if tcp.write_all(buf.get(..n).unwrap()).is_err() {
                                break;
                            }
                        }
                    });
                }
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    Probe { addr, accepted }
}

/// Asserts the answer is the guard's refusal and one refusal was recorded for `host`.
fn assert_guard_refusal(rig: &Rig, host: &str, tcp: &mut TcpStream) {
    let mut rest = String::new();
    let _ = tcp.read_to_string(&mut rest);
    assert!(rest.contains("moor: destination not allowed"), "{rest}");
    let refusals = rig.refusals(1);
    assert_eq!(
        refusals,
        vec![Refusal {
            host: host.to_owned(),
            rule: EGRESS_RULE.to_owned(),
            method: "CONNECT".to_owned(),
            category: GUARD_CATEGORY,
        }]
    );
}

#[test]
fn loopback_targets_are_refused_with_an_empty_guard() {
    let other = probe(false);
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let own = rig.filter.addr().port();
    for port in [own, other.addr.port()] {
        let host = format!("localhost:{port}");
        let (mut tcp, status) = raw_connect(rig.filter.addr(), &host).unwrap();
        assert_eq!(status, 403, "{host}");
        assert_guard_refusal(&rig, "localhost", &mut tcp);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(other.accepted.load(Ordering::SeqCst), 0);
}

#[test]
fn exempt_loopback_is_spliced_both_ways_including_pipelined_bytes() {
    let echo = probe(true);
    let guard = Guard::new(vec![echo.addr]);
    let rig = rig_with(fixed(json_reply("200 OK", OK_BODY)), guard);
    let target = format!("localhost:{}", echo.addr.port());
    let mut tcp = TcpStream::connect_timeout(&rig.filter.addr(), Duration::from_secs(5)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    tcp.write_all(
        format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\nPIPELINED").as_bytes(),
    )
    .unwrap();
    let (mut head, mut byte) = (Vec::new(), [0u8; 1]);
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(tcp.read(&mut byte).unwrap(), 1, "head ended early");
        head.extend_from_slice(&byte);
    }
    assert!(head.starts_with(b"HTTP/1.1 200"), "{head:?}");
    let mut echoed = [0u8; 9];
    tcp.read_exact(&mut echoed).unwrap();
    assert_eq!(&echoed, b"PIPELINED");
    tcp.write_all(b"later").unwrap();
    let mut back = [0u8; 5];
    tcp.read_exact(&mut back).unwrap();
    assert_eq!(&back, b"later");
    assert!(rig.refusals(0).is_empty());
}

#[test]
fn exemption_is_exact_by_port() {
    let exempt = probe(true);
    let other = probe(true);
    let guard = Guard::new(vec![exempt.addr]);
    let rig = rig_with(fixed(json_reply("200 OK", OK_BODY)), guard);
    let host = format!("localhost:{}", other.addr.port());
    let (mut tcp, status) = raw_connect(rig.filter.addr(), &host).unwrap();
    assert_eq!(status, 403);
    assert_guard_refusal(&rig, "localhost", &mut tcp);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(other.accepted.load(Ordering::SeqCst), 0);
}

#[test]
fn refusals_carry_no_payload_bytes() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let host = "10.0.0.1:443";
    let mut tcp = TcpStream::connect_timeout(&rig.filter.addr(), Duration::from_secs(5)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let payload = format!("authorization: token {TOKEN} PAYLOADMARK");
    tcp.write_all(format!("CONNECT {host} HTTP/1.1\r\nHost: {host}\r\n\r\n{payload}").as_bytes())
        .unwrap();
    let mut reply = String::new();
    let _ = tcp.read_to_string(&mut reply);
    assert!(
        !reply.contains(TOKEN) && !reply.contains("PAYLOADMARK"),
        "{reply}"
    );
    let refusals = rig.refusals(1);
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    let shown = format!("{refusals:?}");
    assert!(
        !shown.contains(TOKEN) && !shown.contains("PAYLOADMARK"),
        "{shown}"
    );
    assert_eq!(refusals.first().map(|r| r.host.as_str()), Some("10.0.0.1"));
}
