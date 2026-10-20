//! A Box that names a host resolving to loopback must not reach nono, the filter, the front or any host service through the front.

use std::fmt::Write as _;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::graphql_e2e::{rig_with, Rig, SECRET};

/// An `Authorization` value a Box makes up for a request inside the tunnel.
const MADE_UP: &str = "Authorization: Bearer box-made-up-token";
/// Loopback spellings a Box can name in a `CONNECT` authority, IPv6 bracketed.
const LOOPBACK_TARGETS: [&str; 3] = ["127.0.0.1", "[::1]", "localhost"];
/// A GraphQL query for a repository that is not the Box's.
const OTHER_REPO: &str = r#"{"query":"query{repository(owner:\"victim\",name:\"secrets\"){id}}"}"#;
/// Longest any single read waits.
const IO: Duration = Duration::from_secs(5);

/// Starts the supervisor listener.
fn rig() -> Rig {
    rig_with(Vec::new())
}

/// Reads until the peer closes, a timeout, or `stop` holds for the bytes so far.
fn read_until(tcp: &mut TcpStream, stop: impl Fn(&str) -> bool) -> String {
    tcp.set_read_timeout(Some(IO)).unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    while !stop(&String::from_utf8_lossy(&got)) {
        match tcp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => got.extend_from_slice(buf.get(..n).unwrap_or_default()),
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::ConnectionReset
                ) =>
            {
                break
            }
            Err(e) => unreachable!("read failed: {e}"),
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

/// The status code of the first line of `reply`, 0 when there is none.
fn status(reply: &str) -> u16 {
    reply
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// An origin-form `POST` of the other-repo query to nono's GraphQL route, with extra header lines.
fn graphql_post(extra: &[String]) -> String {
    let mut out = String::from(
        "POST /github-api__0/graphql HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n",
    );
    for h in extra {
        out.push_str(h);
        out.push_str("\r\n");
    }
    let _ = write!(
        out,
        "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{OTHER_REPO}",
        OTHER_REPO.len()
    );
    out
}

/// Sends `CONNECT localhost:<port>` to the front with `auth` lines; if a tunnel opens, replays the inner request.
/// Returns the CONNECT status and whatever came back through a tunnel.
fn hairpin(front: u16, port: u16, auth: &[String]) -> (u16, String) {
    hairpin_to(front, "localhost", port, auth)
}

/// Like `hairpin`, naming `host` in the `CONNECT` authority.
fn hairpin_to(front: u16, host: &str, port: u16, auth: &[String]) -> (u16, String) {
    let mut tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, front)).unwrap();
    tcp.set_write_timeout(Some(IO)).unwrap();
    let mut head = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    for h in auth {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    tcp.write_all(head.as_bytes()).unwrap();
    let reply = read_until(&mut tcp, |t| t.contains("\r\n\r\n"));
    let code = status(&reply);
    let inner = if (200..300).contains(&code) {
        let _ = tcp.write_all(graphql_post(&[MADE_UP.to_owned()]).as_bytes());
        read_until(&mut tcp, |_| false)
    } else {
        String::new()
    };
    (code, inner)
}

/// A loopback listener that counts the connections it accepts.
struct Service {
    /// Where it listens.
    port: u16,
    /// Connections accepted so far.
    hits: Arc<AtomicUsize>,
    /// Tells the accept loop to end.
    stop: Arc<AtomicBool>,
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Starts a host service that accepts and drops connections.
fn service() -> Service {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (hits2, stop2) = (Arc::clone(&hits), Arc::clone(&stop));
    std::thread::spawn(move || {
        while !stop2.load(Ordering::SeqCst) {
            if listener.accept().is_ok() {
                hits2.fetch_add(1, Ordering::SeqCst);
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    Service { port, hits, stop }
}

/// Every event payload recorded for the Box, as text, after a drain.
fn payloads(rig: &Rig) -> Vec<(String, String)> {
    rig.live.as_ref().unwrap().drain(&rig.deps(), rig.box_id);
    rig.store
        .events(rig.box_id, 100)
        .unwrap()
        .into_iter()
        .map(|e| (e.kind, e.payload.to_string()))
        .collect()
}

/// Nothing recorded may carry a credential.
fn assert_no_leak(events: &[(String, String)]) {
    for (kind, payload) in events {
        for leak in [SECRET, "box-made-up-token", "Bearer ", "Basic "] {
            assert!(
                !payload.contains(leak),
                "{leak} leaked into {kind}: {payload}"
            );
        }
    }
}

/// The hairpin ended with a refusal and no request reached the fake `api.github.com`.
fn assert_blocked(rig: &Rig, outcome: (u16, String)) {
    let (code, inner) = outcome;
    assert!(rig.seen().is_empty(), "upstream saw {:?}", rig.seen());
    assert!(!(200..300).contains(&code), "tunnel opened: {code} {inner}");
    assert!(!inner.contains("R_1"), "inner reply came back: {inner}");
}

/// Polls the recorded events until a `destination_guard` refusal appears.
fn guard_refusals(rig: &Rig) -> Vec<(String, String)> {
    let mut events = Vec::new();
    for _ in 0..100 {
        events = payloads(rig);
        if events
            .iter()
            .any(|(k, p)| k == "egress_refused" && p.contains("destination_guard"))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    events
}

/// A `CONNECT` to `host:<nono port>` is refused by the destination guard, not by nono.
fn nono_port_refused(host: &str) {
    let rig = rig();
    let live = rig.live.as_ref().unwrap();
    let outcome = hairpin_to(rig.box_port, host, live.nono_port(), &[]);
    assert_blocked(&rig, outcome);
    let events = guard_refusals(&rig);
    assert!(
        events
            .iter()
            .any(|(k, p)| k == "egress_refused" && p.contains("destination_guard")),
        "{host}: no destination_guard refusal in {events:?}"
    );
    assert_no_leak(&events);
    rig.live.unwrap().stop();
}

/// A `CONNECT` to `host:<port>` of every loopback listener is refused and none is dialled.
fn every_listener_refused(host: &str) {
    let rig = rig();
    let live = rig.live.as_ref().unwrap();
    let svc = service();
    let targets = [
        live.graphql_addr().unwrap().port(),
        rig.box_port,
        live.nono_port(),
        svc.port,
    ];
    for port in targets {
        let (code, inner) = hairpin_to(rig.box_port, host, port, &[]);
        assert!(
            rig.seen().is_empty(),
            "{host}:{port}: upstream saw {:?}",
            rig.seen()
        );
        assert!(
            !(200..300).contains(&code),
            "{host}:{port}: tunnel opened: {code} {inner}"
        );
        assert!(
            !inner.contains("R_1"),
            "{host}:{port}: inner reply came back: {inner}"
        );
    }
    let events = guard_refusals(&rig);
    assert!(
        events
            .iter()
            .any(|(k, p)| k == "egress_refused" && p.contains("destination_guard")),
        "{host}: no destination_guard refusal in {events:?}"
    );
    assert_eq!(
        svc.hits.load(Ordering::SeqCst),
        0,
        "{host}: host service was dialled"
    );
    rig.live.unwrap().stop();
}

#[test]
fn connect_to_the_nono_port_is_refused_and_nothing_reaches_github() {
    for host in LOOPBACK_TARGETS {
        nono_port_refused(host);
    }
}

#[test]
fn connect_to_every_loopback_listener_is_refused() {
    for host in LOOPBACK_TARGETS {
        every_listener_refused(host);
    }
}

#[test]
fn what_the_box_can_send_is_not_accepted_by_nono_directly() {
    let rig = rig();
    let variants = [
        Vec::new(),
        vec![MADE_UP.to_owned()],
        vec!["Proxy-Authorization: Basic bW9vcjp4".to_owned()],
        vec![
            MADE_UP.to_owned(),
            "Proxy-Authorization: Bearer box-made-up-token".to_owned(),
        ],
    ];
    for extra in variants {
        let mut tcp =
            TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, rig.nono_port()))).unwrap();
        tcp.set_write_timeout(Some(IO)).unwrap();
        tcp.write_all(graphql_post(&extra).as_bytes()).unwrap();
        let reply = read_until(&mut tcp, |_| false);
        let code = status(&reply);
        assert!(matches!(code, 401 | 403 | 407), "nono answered {code}");
        assert!(!reply.contains("R_1") && !reply.contains(SECRET));
        assert!(rig.seen().is_empty(), "upstream saw {:?}", rig.seen());
    }
    rig.live.unwrap().stop();
}

#[test]
fn refused_hairpin_is_audited_as_a_destination_guard_refusal_without_secrets() {
    let rig = rig();
    let live = rig.live.as_ref().unwrap();
    let outcome = hairpin(rig.box_port, live.nono_port(), &[]);
    assert_blocked(&rig, outcome);
    let mut events = Vec::new();
    for _ in 0..100 {
        events = payloads(&rig);
        if events.iter().any(|(_, p)| p.contains("destination_guard")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        events
            .iter()
            .any(|(k, p)| k == "egress_refused" && p.contains("destination_guard")),
        "no destination_guard refusal in {events:?}"
    );
    assert_no_leak(&events);
    rig.live.unwrap().stop();
}
