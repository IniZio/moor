//! Tests for stopping the server: in-flight requests finish, new ones are refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions unwrap I/O"
)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use super::*;

/// Token the test servers accept.
const TOKEN: &str = "t0ken";

/// A complete authenticated request.
const GOOD: &str = "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer t0ken\r\n\r\n";

/// A server whose handler sets `entered`, waits `work`, sets `finished`, then answers a page.
fn slow_server(entered: &Arc<AtomicBool>, finished: &Arc<AtomicBool>, work: Duration) -> Server {
    let (entered, finished) = (Arc::clone(entered), Arc::clone(finished));
    let handler: Handler = Arc::new(move |_| {
        entered.store(true, Ordering::SeqCst);
        thread::sleep(work);
        finished.store(true, Ordering::SeqCst);
        HttpResponse::html("<h1>done</h1>").into()
    });
    start(0, TOKEN.into(), handler).unwrap()
}

/// Sends a request on a new connection and returns the stream once the handler is running.
fn in_flight(web: &Server, entered: &AtomicBool) -> TcpStream {
    let mut s = TcpStream::connect(web.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(GOOD.as_bytes()).unwrap();
    while !entered.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(10));
    }
    s
}

#[test]
fn shutdown_waits_for_the_request_in_flight() {
    let entered = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let web = slow_server(&entered, &finished, Duration::from_millis(600));
    let mut client = in_flight(&web, &entered);
    web.shutdown(Duration::from_secs(10));
    assert!(finished.load(Ordering::SeqCst), "shutdown returned early");
    let mut out = String::new();
    client.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 200 OK"), "got {out:?}");
    assert!(out.ends_with("<h1>done</h1>"), "got {out:?}");
}

#[test]
fn shutdown_stops_accepting() {
    let entered = Arc::new(AtomicBool::new(false));
    let web = slow_server(&entered, &Arc::default(), Duration::ZERO);
    let addr = web.addr();
    web.shutdown(Duration::from_secs(10));
    assert!(TcpStream::connect(addr).is_err());
}

#[test]
fn shutdown_gives_up_on_a_request_that_outlasts_the_grace() {
    let entered = Arc::new(AtomicBool::new(false));
    let web = slow_server(&entered, &Arc::default(), Duration::from_secs(5));
    let _client = in_flight(&web, &entered);
    let start = limits::now();
    web.shutdown(Duration::from_millis(200));
    assert!(start.elapsed() < Duration::from_secs(3));
}
