use std::io::{ErrorKind, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use moor_core::seam::clock::Clock;

use super::DeadlineReader;
use crate::adapters::clock::SystemClock;
use crate::proxy::head::{read_head, HeadError};

/// Returns the connected (client, server) ends of a loopback connection.
fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let client = TcpStream::connect(listener.local_addr().expect("addr")).expect("connect");
    let (server, _) = listener.accept().expect("accept");
    (client, server)
}

#[test]
fn trickled_head_times_out_at_the_deadline() {
    let (mut client, server) = pair();
    let writer = thread::spawn(move || {
        for _ in 0..60 {
            if client.write_all(b"G").is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });
    let clock = SystemClock::real();
    let start = clock.instant();
    let mut reader = DeadlineReader::new(&server, &clock, Duration::from_millis(300));
    let result = read_head(&mut reader);
    let elapsed = clock.instant().saturating_duration_since(start);
    drop(server);
    writer.join().expect("writer");
    assert!(
        matches!(&result, Err(HeadError::Io(e)) if e.kind() == ErrorKind::TimedOut),
        "expected a timed-out read"
    );
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
}

#[test]
fn head_within_budget_is_read() {
    let (mut client, server) = pair();
    client
        .write_all(b"GET /v1/x HTTP/1.1\r\nHost: a\r\n\r\n")
        .expect("write");
    let clock = SystemClock::real();
    let mut reader = DeadlineReader::new(&server, &clock, Duration::from_secs(2));
    assert!(matches!(read_head(&mut reader), Ok(Some(_))));
}

#[test]
fn expired_budget_fails_without_reading() {
    let (_client, server) = pair();
    let clock = SystemClock::real();
    let mut reader = DeadlineReader::new(&server, &clock, Duration::ZERO);
    let err = std::io::Read::read(&mut reader, &mut [0u8; 4]).expect_err("expired");
    assert_eq!(err.kind(), ErrorKind::TimedOut);
}
