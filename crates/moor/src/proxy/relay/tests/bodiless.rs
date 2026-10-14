//! Responses without a body are relayed without added framing.

use std::io::Write;

use moor_core::proxy_gate::Inject;

use super::{forward, read_raw, read_request, relay, spawn, upstream};

/// Relays `method` against an upstream sending `reply`; returns what the
/// client received, whether its read failed, and the returned status.
fn relayed(method: &'static str, reply: &'static [u8]) -> (String, bool, u16) {
    let (port, up) = upstream(move |mut s| {
        read_request(&mut s);
        s.write_all(reply).unwrap();
    });
    let req = forward("model", method, Inject::Bearer, &[], b"");
    let (mut client, done) = spawn(relay(port, true), req);
    let (raw, failed) = read_raw(&mut client);
    let status = done.join().unwrap();
    up.join().unwrap();
    (String::from_utf8_lossy(&raw).into_owned(), failed, status)
}

/// A `HEAD` response keeps its `content-length` and ends cleanly.
#[test]
fn head_forwards_length_and_ends_cleanly() {
    let (text, failed, status) = relayed("HEAD", b"HTTP/1.1 200 OK\r\ncontent-length: 42\r\n\r\n");
    assert!(!failed, "{text}");
    assert_eq!(status, 200, "{text}");
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
    assert!(text.contains("content-length: 42\r\n"), "{text}");
    assert!(!text.contains("transfer-encoding"), "{text}");
    assert!(text.ends_with("\r\n\r\n"), "{text}");
}

/// A `304` keeps the representation's `content-length` and ends cleanly.
#[test]
fn not_modified_forwards_length_and_ends_cleanly() {
    let (text, failed, status) = relayed(
        "GET",
        b"HTTP/1.1 304 Not Modified\r\ncontent-length: 42\r\n\r\n",
    );
    assert!(!failed, "{text}");
    assert_eq!(status, 304, "{text}");
    assert!(text.starts_with("HTTP/1.1 304 Not Modified\r\n"), "{text}");
    assert!(text.contains("content-length: 42\r\n"), "{text}");
    assert!(text.ends_with("\r\n\r\n"), "{text}");
}

/// A `204` gets no chunked framing and no body bytes.
#[test]
fn no_content_has_no_framing_or_body() {
    let (text, failed, status) = relayed("GET", b"HTTP/1.1 204 No Content\r\n\r\n");
    assert!(!failed, "{text}");
    assert_eq!(status, 204, "{text}");
    assert!(!text.contains("transfer-encoding"), "{text}");
    assert!(text.ends_with("\r\n\r\n"), "{text}");
    assert!(!text.contains("0\r\n\r\n0"), "{text}");
    assert_eq!(text.split("\r\n\r\n").nth(1), Some(""), "{text}");
}
