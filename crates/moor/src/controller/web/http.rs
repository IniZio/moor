//! Minimal HTTP/1.1 request parsing and response writing for the web UI.

use std::io::{self, Read, Write};

/// Largest request head accepted, in bytes.
const MAX_HEAD: usize = 16 * 1024;

/// Headers added to every response.
const COMMON_HEADERS: &str = "Connection: close\r\n\
X-Content-Type-Options: nosniff\r\n\
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self'\r\n\
Referrer-Policy: no-referrer\r\n";

/// A parsed request head.
pub(super) struct Request {
    /// Request method, as sent.
    pub(super) method: String,
    /// Path without the query string.
    pub(super) path: String,
    /// Raw query string, empty when absent.
    query: String,
    /// Header names (lowercased) with their values.
    headers: Vec<(String, String)>,
}

impl Request {
    /// Value of the header `name` (lowercase), if sent.
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Value of the query parameter `key`; values are taken verbatim, not percent-decoded.
    pub(super) fn query_param(&self, key: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    /// Value of the cookie `name`, if sent.
    pub(super) fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?
            .split(';')
            .filter_map(|kv| kv.trim().split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    }
}

/// Reads and parses one request head.
///
/// # Errors
/// Fails on I/O errors or timeout, an oversized head, early EOF or a malformed head.
pub(super) fn read_request(stream: &mut impl Read) -> io::Result<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > MAX_HEAD {
            return Err(invalid("request head too large"));
        }
        let n = stream.read(&mut chunk)?;
        let got = chunk.get(..n).unwrap_or_default();
        if got.is_empty() {
            return Err(invalid("connection closed before end of head"));
        }
        buf.extend_from_slice(got);
    }
    parse_head(&String::from_utf8_lossy(&buf))
}

/// Parses the text of a request head.
fn parse_head(text: &str) -> io::Result<Request> {
    let mut lines = text.split("\r\n");
    let mut parts = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(invalid("malformed request line"));
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    Ok(Request {
        method: method.to_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
        headers,
    })
}

/// An `InvalidData` error.
fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// Writes a complete response with a fixed body.
///
/// # Errors
/// Fails when the write fails.
pub(super) fn respond(
    out: &mut impl Write,
    status: &str,
    content_type: &str,
    extra: &[(&str, &str)],
    body: &str,
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{COMMON_HEADERS}",
        body.len()
    );
    push_extra(&mut head, extra);
    head.push_str("\r\n");
    head.push_str(body);
    out.write_all(head.as_bytes())?;
    out.flush()
}

/// Writes the head of an open-ended `text/event-stream` response.
///
/// # Errors
/// Fails when the write fails.
pub(super) fn begin_stream(out: &mut impl Write) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n{COMMON_HEADERS}\r\n"
    );
    out.write_all(head.as_bytes())?;
    out.flush()
}

/// Appends `extra` headers to `head`.
fn push_extra(head: &mut String, extra: &[(&str, &str)]) {
    for (k, v) in extra {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
}
