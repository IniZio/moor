//! Many requests held at once: past [`HOLD_CAP`] they are turned away at once with a way back.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use moor_gate::{HOLD, HOLD_CAP};

use super::{held_with, CLIENT_ID, FRONT_KEY};

/// Each socket in `open` that has its response head, removed from `open`.
fn answered_heads(open: &mut Vec<(TcpStream, Vec<u8>)>) -> Vec<String> {
    let mut heads = Vec::new();
    open.retain_mut(|(stream, head)| {
        let mut buf = [0; 1024];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            head.extend_from_slice(&buf[..n]);
        }
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            heads.push(String::from_utf8_lossy(head).into_owned());
            return false;
        }
        true
    });
    heads
}

/// Every socket's response head, once `count` more have one, or panics at `bound`.
fn collect_heads(
    open: &mut Vec<(TcpStream, Vec<u8>)>,
    count: usize,
    bound: Duration,
) -> Vec<String> {
    let began = Instant::now();
    let mut heads = Vec::new();
    while heads.len() < count {
        assert!(
            began.elapsed() < bound,
            "{} of {count} answered",
            heads.len()
        );
        heads.extend(answered_heads(open));
        std::thread::sleep(Duration::from_millis(20));
    }
    heads
}

#[test]
fn past_the_cap_a_request_gets_503_at_once_and_the_ones_held_still_go_through() {
    const PAST: usize = 20;
    let mut w = held_with(FRONT_KEY, HOLD);
    w.away();
    let token = w.token(CLIENT_ID);
    let mut open: Vec<_> = (0..HOLD_CAP + PAST)
        .map(|_| {
            let mut stream = TcpStream::connect(&w.gate).unwrap();
            write!(
                stream,
                "GET /front HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
            )
            .unwrap();
            stream.set_nonblocking(true).unwrap();
            (stream, Vec::new())
        })
        .collect();
    let refused = collect_heads(&mut open, PAST, Duration::from_secs(20));
    for head in &refused {
        assert!(head.starts_with("HTTP/1.1 503"), "{head}");
        assert!(head.contains("Retry-After: 5\r\n"), "{head}");
    }
    assert_eq!(open.len(), HOLD_CAP, "more than the cap were turned away");
    let up = w.up_again();
    for head in collect_heads(&mut open, HOLD_CAP, Duration::from_secs(20)) {
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    }
    up.stop();
}
