//! Tests for the request-read deadline and the connection cap over a live server.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions unwrap I/O"
)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use super::*;

/// Token the test servers accept.
const TOKEN: &str = "t0ken";

/// A complete authenticated request.
const GOOD: &str = "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer t0ken\r\n\r\n";

/// Starts a server with `limits` whose handler serves a fixed page.
fn server(limits: Limits) -> Server {
    let handler: Handler = Arc::new(|_| HttpResponse::html("<h1>page</h1>"));
    start_with(0, TOKEN.into(), handler, limits).unwrap()
}

/// Limits with a short deadline and the given cap.
const fn limits(deadline_ms: u64, max: usize) -> Limits {
    Limits {
        read_deadline: Duration::from_millis(deadline_ms),
        max_connections: max,
    }
}

/// Opens a connection that reads for at most five seconds.
fn connect(web: &Server) -> TcpStream {
    let s = TcpStream::connect(web.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s
}

/// Sends `GOOD` and returns whatever the server answers before closing.
fn fetch(web: &Server) -> String {
    let mut s = connect(web);
    s.write_all(GOOD.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn normal_request_succeeds_under_limits() {
    let web = server(limits(300, 2));
    assert!(fetch(&web).starts_with("HTTP/1.1 200 OK"));
}

#[test]
fn trickling_client_is_cut_off_at_the_deadline() {
    let web = server(limits(300, 2));
    let mut s = connect(&web);
    s.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    let start = limits::now();
    let mut out = String::new();
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(100));
        if s.write_all(b"X").is_err() {
            break;
        }
        s.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
        let mut buf = [0u8; 512];
        if let Ok(n) = s.read(&mut buf) {
            out.push_str(&String::from_utf8_lossy(buf.get(..n).unwrap_or_default()));
            break;
        }
    }
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "not cut off in time"
    );
    assert!(
        out.is_empty() || out.starts_with("HTTP/1.1 408"),
        "got {out:?}"
    );
}

#[test]
fn third_connection_over_the_cap_gets_503() {
    let web = server(limits(5000, 2));
    let (_a, _b) = (connect(&web), connect(&web));
    let mut third = connect(&web);
    let mut out = String::new();
    let _ = third.read_to_string(&mut out);
    assert!(
        out.starts_with("HTTP/1.1 503 Service Unavailable"),
        "got {out:?}"
    );
    assert!(out.contains("Retry-After: 1"));
    assert!(out.contains("Connection: close"));
}

#[test]
fn slot_is_released_when_a_connection_closes() {
    let web = server(limits(5000, 2));
    let (a, _b) = (connect(&web), connect(&web));
    drop(a);
    let deadline = limits::now() + Duration::from_secs(3);
    loop {
        let out = fetch(&web);
        if out.starts_with("HTTP/1.1 200 OK") {
            break;
        }
        assert!(out.starts_with("HTTP/1.1 503"), "got {out:?}");
        assert!(limits::now() < deadline, "slot never freed");
        thread::sleep(Duration::from_millis(50));
    }
}
