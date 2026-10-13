//! Tests for the loopback proxy listener.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use moor_core::proxy_gate::{RouteRow, RouteTable};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{Secret, TaskId};

use super::framing::body_cap;
use super::{Forward, ForwardHandler, Proxy, ProxyOpts, HEAD_BUDGET};
use crate::adapters::clock::SystemClock;
use crate::proxy::ProxyLog;
use crate::testing::clock::FakeClock;

const NONCE: &str = "q3Jt0bXo1n9Zr2Kc8vWm4yHs7aLp6eTg5uDf0iBk-_A";
const OK: &str = "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

/// Records each admitted request and answers 200.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<(String, String)>>,
    /// Length of the body bytes each call received with its head.
    prefixes: Mutex<Vec<usize>>,
}

impl ForwardHandler for Fake {
    fn forward(&self, req: Forward, stream: &mut TcpStream) -> u16 {
        self.prefixes.lock().unwrap().push(req.body_prefix.len());
        self.calls
            .lock()
            .unwrap()
            .push((req.head.method, req.plan.upstream_path));
        stream.write_all(OK.as_bytes()).unwrap();
        200
    }
}

/// Blocks until released, to hold a connection slot.
struct Slow;

impl ForwardHandler for Slow {
    fn forward(&self, _req: Forward, stream: &mut TcpStream) -> u16 {
        thread::sleep(Duration::from_millis(600));
        stream.write_all(OK.as_bytes()).unwrap();
        200
    }
}

struct Rig {
    proxy: Proxy,
    dir: tempfile::TempDir,
    fake: Arc<Fake>,
}

fn start_with(handler: Arc<dyn ForwardHandler>, dir: &Path, max: usize) -> Proxy {
    start_forge(
        handler,
        dir,
        RouteTable::new(vec![RouteRow::model()]),
        opts(max, None),
    )
}

/// Settings with the real clock and a generous head budget.
fn opts(max_conns: usize, task_repo: Option<String>) -> ProxyOpts {
    ProxyOpts {
        max_conns,
        task_repo,
        clock: Arc::new(SystemClock::real()),
        head_budget: HEAD_BUDGET,
        body_cap,
    }
}

fn start_forge(
    handler: Arc<dyn ForwardHandler>,
    dir: &Path,
    table: RouteTable,
    opts: ProxyOpts,
) -> Proxy {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let log = ProxyLog::open(dir, TaskId::new(1), clock).unwrap();
    let nonce = Secret::new(NONCE.as_bytes().to_vec());
    Proxy::start(table, nonce, log, handler, opts).unwrap()
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default());
    let proxy = start_with(fake.clone(), dir.path(), 8);
    Rig { proxy, dir, fake }
}

fn send(port: u16, request: &[u8]) -> String {
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    s.write_all(request).unwrap();
    let mut out = Vec::new();
    drop(s.read_to_end(&mut out));
    String::from_utf8_lossy(&out).into_owned()
}

fn authed(path: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nhost: x\r\nauthorization: Bearer {NONCE}\r\n\r\n").into_bytes()
}

fn read_logs(dir: &Path) -> String {
    let mut all = String::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all.push_str(&read_logs(&path));
        } else {
            all.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
    }
    all
}

/// Waits until the log has `lines` lines, then returns its text.
fn log_when(dir: &Path, lines: usize) -> String {
    for _ in 0..500 {
        let text = read_logs(dir);
        if text.lines().count() >= lines {
            return text;
        }
        thread::sleep(Duration::from_millis(10));
    }
    read_logs(dir)
}

#[test]
fn it_listens_on_loopback() {
    let r = rig();
    let s = TcpStream::connect((Ipv4Addr::LOCALHOST, r.proxy.port())).unwrap();
    assert!(s.peer_addr().unwrap().ip().is_loopback());
    let other = std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));
    // A wildcard bind would also answer on every other loopback address.
    assert!(TcpStream::connect((other, r.proxy.port())).is_err());
    r.proxy.shutdown();
}

#[test]
fn it_refuses_unauthenticated_requests_without_echoing_them() {
    let r = rig();
    let reply = send(
        r.proxy.port(),
        b"GET /distinctive-path HTTP/1.1\r\nx-distinct: marker-value\r\n\r\n",
    );
    assert_eq!(
        reply,
        "HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain\r\ncontent-length: 13\r\nconnection: close\r\n\r\nunauthorized\n"
    );
    assert!(!reply.contains("distinctive") && !reply.contains("marker"));
    assert!(r.fake.calls.lock().unwrap().is_empty());
    r.proxy.shutdown();
}

#[test]
fn it_hands_authorized_requests_to_the_handler_and_logs_them() {
    let r = rig();
    let reply = send(r.proxy.port(), &authed("/v1/messages"));
    assert_eq!(reply, OK);
    assert_eq!(
        *r.fake.calls.lock().unwrap(),
        vec![("GET".to_owned(), "/v1/messages".to_owned())]
    );
    let log = log_when(r.dir.path(), 1);
    assert!(log.trim_end().ends_with(" GET model 200"), "{log}");
    r.proxy.shutdown();
}

#[test]
fn it_rejects_too_many_headers_and_oversized_heads() {
    let r = rig();
    let headers = (0..70)
        .map(|i| format!("h{i}: v\r\n"))
        .collect::<Vec<_>>()
        .concat();
    let many = format!("GET / HTTP/1.1\r\n{headers}\r\n");
    let reply = send(r.proxy.port(), many.as_bytes());
    assert!(reply.starts_with("HTTP/1.1 431 "), "{reply}");
    assert!(reply.ends_with("request header fields too large\n"));

    let big = format!("GET / HTTP/1.1\r\nx: {}\r\n\r\n", "a".repeat(20 * 1024));
    let reply = send(r.proxy.port(), big.as_bytes());
    assert!(reply.starts_with("HTTP/1.1 431 "), "{reply}");
    let log = log_when(r.dir.path(), 2);
    assert_eq!(
        log.lines().filter(|l| l.ends_with(" - - 431")).count(),
        2,
        "{log}"
    );
    r.proxy.shutdown();
}

#[test]
fn it_sends_continue_only_to_admitted_requests() {
    let r = rig();
    let req = format!(
        "POST /v1/x HTTP/1.1\r\nauthorization: Bearer {NONCE}\r\nexpect: 100-continue\r\n\r\n"
    );
    let reply = send(r.proxy.port(), req.as_bytes());
    assert_eq!(reply, format!("HTTP/1.1 100 Continue\r\n\r\n{OK}"));
    let denied = send(
        r.proxy.port(),
        b"POST /v1/x HTTP/1.1\r\nexpect: 100-continue\r\n\r\n",
    );
    assert!(denied.starts_with("HTTP/1.1 401 "), "{denied}");
    assert!(!denied.contains("100 Continue"));
    r.proxy.shutdown();
}

#[test]
fn it_survives_clients_that_connect_and_drop() {
    let r = rig();
    drop(TcpStream::connect((Ipv4Addr::LOCALHOST, r.proxy.port())).unwrap());
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, r.proxy.port())).unwrap();
    s.write_all(b"GET / HT").unwrap();
    drop(s);
    assert_eq!(send(r.proxy.port(), &authed("/ok")), OK);
    let log = log_when(r.dir.path(), 1);
    assert!(log.lines().any(|l| l.ends_with(" GET model 200")), "{log}");
    r.proxy.shutdown();
}

#[test]
fn it_answers_busy_over_the_connection_cap() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_with(Arc::new(Slow), dir.path(), 1);
    let port = proxy.port();
    let first = thread::spawn(move || send(port, &authed("/slow")));
    thread::sleep(Duration::from_millis(200));
    let reply = send(port, &authed("/second"));
    assert!(reply.starts_with("HTTP/1.1 503 "), "{reply}");
    assert!(reply.ends_with("busy\n"));
    assert_eq!(first.join().unwrap(), OK);
    proxy.shutdown();
}

#[test]
fn it_never_logs_the_nonce_or_header_data() {
    let r = rig();
    send(r.proxy.port(), &authed(&format!("/t/{NONCE}/v1/x")));
    send(r.proxy.port(), &authed("/v1/y"));
    send(
        r.proxy.port(),
        format!("GET /t/{NONCE}/nope HTTP/1.1\r\nx-secret-header: hunter2\r\n\r\n").as_bytes(),
    );
    send(
        r.proxy.port(),
        b"GET /v1/z HTTP/1.1\r\nx-secret-header: hunter2\r\n\r\n",
    );
    let log = log_when(r.dir.path(), 4);
    assert_eq!(log.lines().count(), 4, "{log}");
    for needle in [
        NONCE,
        "hunter2",
        "x-secret-header",
        "authorization",
        "Bearer",
        "/v1/",
    ] {
        assert!(
            !log.to_lowercase().contains(&needle.to_lowercase()),
            "{needle} in {log}"
        );
    }
    r.proxy.shutdown();
}

#[test]
fn it_admits_git_only_for_the_task_repo() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default());
    let table = RouteTable::new(vec![RouteRow::forge_git("/github")]);
    let proxy = start_forge(
        fake.clone(),
        dir.path(),
        table,
        opts(8, Some("o/n".to_owned())),
    );
    let git = |repo: &str| {
        let path = format!("/t/{NONCE}/github/{repo}.git/info/refs?service=git-upload-pack");
        send(proxy.port(), &authed(&path))
    };
    assert_eq!(git("o/n"), OK);
    assert!(git("x/y").starts_with("HTTP/1.1 403"));
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    proxy.shutdown();
}

/// Reads one more byte from the client, as a handler reading a body would.
struct ReadsByte;

impl ForwardHandler for ReadsByte {
    fn forward(&self, _req: Forward, stream: &mut TcpStream) -> u16 {
        let mut byte = [0_u8; 1];
        if stream.read_exact(&mut byte).is_err() {
            return 500;
        }
        stream.write_all(OK.as_bytes()).unwrap();
        200
    }
}

#[test]
fn it_closes_clients_that_trickle_the_head_past_the_budget() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default());
    let mut o = opts(8, None);
    o.head_budget = Duration::from_millis(300);
    let table = RouteTable::new(vec![RouteRow::model()]);
    let proxy = start_forge(fake.clone(), dir.path(), table, o);
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port())).unwrap();
    let mut writer = s.try_clone().unwrap();
    let trickle = thread::spawn(move || {
        if writer.write_all(b"GET / HTTP/1.1\r\nx-pad: ").is_err() {
            return;
        }
        for _ in 0..60 {
            if writer.write_all(b"a").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let clock = SystemClock::real();
    let started = clock.instant();
    let mut out = Vec::new();
    drop(s.read_to_end(&mut out));
    assert!(clock.instant().duration_since(started) < Duration::from_secs(2));
    assert!(out.is_empty());
    assert!(fake.calls.lock().unwrap().is_empty());
    drop(s);
    drop(trickle.join());
    proxy.shutdown();
}

#[test]
fn it_leaves_no_head_timeout_on_the_stream_given_to_the_handler() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(8, None);
    o.head_budget = Duration::from_millis(300);
    let table = RouteTable::new(vec![RouteRow::model()]);
    let proxy = start_forge(Arc::new(ReadsByte), dir.path(), table, o);
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port())).unwrap();
    let head = format!(
        "GET /v1/messages HTTP/1.1\r\nhost: x\r\ncontent-length: 1\r\nauthorization: Bearer {NONCE}\r\n\r\n"
    );
    s.write_all(head.as_bytes()).unwrap();
    // The body arrives after the head budget has passed.
    thread::sleep(Duration::from_millis(400));
    s.write_all(b"x").unwrap();
    let mut out = Vec::new();
    drop(s.read_to_end(&mut out));
    assert_eq!(String::from_utf8_lossy(&out), OK);
    proxy.shutdown();
}

#[test]
fn it_refuses_ambiguous_body_framing_before_the_handler() {
    let r = rig();
    let cases: [&[&str]; 5] = [
        &["content-length: 5", "content-length: 5"],
        &["content-length: 5, 6"],
        &["transfer-encoding: chunked", "content-length: 5"],
        &["transfer-encoding: gzip"],
        &["content-length: abc"],
    ];
    for headers in cases {
        let req = format!(
            "POST /v1/x HTTP/1.1\r\nauthorization: Bearer {NONCE}\r\n{}\r\n\r\nhello",
            headers.join("\r\n")
        );
        let reply = send(r.proxy.port(), req.as_bytes());
        assert!(reply.starts_with("HTTP/1.1 400 "), "{headers:?}: {reply}");
    }
    assert!(r.fake.calls.lock().unwrap().is_empty());
    let log = log_when(r.dir.path(), 5);
    assert_eq!(
        log.lines().filter(|l| l.ends_with(" POST - 400")).count(),
        5,
        "{log}"
    );
    r.proxy.shutdown();
}

#[test]
fn it_delivers_the_refusal_even_when_the_client_sent_a_large_body() {
    let r = rig();
    let mut req = format!(
        "POST /v1/x HTTP/1.1\r\nauthorization: Bearer {NONCE}\r\ncontent-length: abc\r\n\r\n"
    )
    .into_bytes();
    req.extend(std::iter::repeat_n(b'x', 48 * 1024));
    let reply = send(r.proxy.port(), &req);
    assert!(reply.starts_with("HTTP/1.1 400 "), "{reply}");
    r.proxy.shutdown();
}

#[test]
fn it_refuses_declared_bodies_over_the_route_cap() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default());
    let mut o = opts(8, None);
    o.body_cap = |_| 10;
    let table = RouteTable::new(vec![RouteRow::model()]);
    let proxy = start_forge(fake.clone(), dir.path(), table, o);
    let post = |len: usize, body: usize| {
        let req = format!(
            "POST /v1/x HTTP/1.1\r\nauthorization: Bearer {NONCE}\r\ncontent-length: {len}\r\n\r\n{}",
            "x".repeat(body)
        );
        send(proxy.port(), req.as_bytes())
    };
    let over = post(11, 11);
    assert!(over.starts_with("HTTP/1.1 413 "), "{over}");
    assert!(over.ends_with("content too large\n"));
    assert!(fake.calls.lock().unwrap().is_empty());
    assert_eq!(post(10, 10), OK);
    assert_eq!(*fake.prefixes.lock().unwrap(), vec![10]);
    let log = log_when(dir.path(), 2);
    assert!(log.lines().any(|l| l.ends_with(" POST model 413")), "{log}");
    proxy.shutdown();
}
