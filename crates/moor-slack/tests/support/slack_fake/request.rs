//! Reading one HTTP request the fake Web API received.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::io::Read;
use std::net::TcpStream;

use serde_json::Value;

use super::DEADLINE;

/// Reads the request line, headers and body; returns path, authorization and JSON body.
pub(super) fn read_request(stream: &mut TcpStream) -> Option<(String, String, Value)> {
    stream.set_read_timeout(Some(DEADLINE)).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 1024];
    let head_end = loop {
        if let Some(at) = find(&buf, b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines.next()?.split(' ').nth(1)?.to_owned();
    let header = |name: &str| {
        lines
            .clone()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_owned())
    };
    let auth = header("authorization").unwrap_or_default();
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        body.extend_from_slice(&chunk[..n]);
    }
    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Some((path, auth, body))
}

/// Index of the first occurrence of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
