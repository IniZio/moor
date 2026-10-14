//! Writes the upstream response to the client with honest framing.
//!
//! A body that ends early is never presented as complete: the client either
//! gets a connection reset or a chunked body without its terminating chunk.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use ureq::http::{HeaderName, Response};
use ureq::Body;

/// Size of the buffer the response body is copied through.
const COPY_BUF: usize = 8 * 1024;
/// Response headers that never cross the proxy.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// How a response ended.
pub(super) enum Written {
    /// The whole response was sent; carries the upstream status.
    Complete(u16),
    /// The response was cut short and the client connection was reset.
    Aborted,
}

/// How the response body is delimited toward the client.
#[derive(Clone, Copy)]
enum Out {
    /// The upstream's `content-length`, forwarded as is.
    Length(u64),
    /// Re-chunked, because the upstream gave no length.
    Chunked,
    /// No body follows the head; a `content-length` is only forwarded.
    Bodiless(Option<u64>),
}

/// Whether a response to `method` with `code` carries no body.
const fn is_bodiless(method: &str, code: u16) -> bool {
    method.eq_ignore_ascii_case("HEAD") || matches!(code, 100..=199 | 204 | 304)
}

/// Writes the upstream response head and streams its body.
///
/// Responses to `HEAD` and `1xx`, `204` and `304` responses have no body: their
/// `content-length` describes the representation and is forwarded as is, and
/// no framing is added.
pub(super) fn write_response(
    stream: &mut TcpStream,
    response: Response<Body>,
    method: &str,
) -> Written {
    let (parts, mut body) = response.into_parts();
    let code = parts.status.as_u16();
    let named: Vec<String> = parts
        .headers
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .collect();
    let length = parts
        .headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let out = if is_bodiless(method, code) {
        Out::Bodiless(length)
    } else {
        length.map_or(Out::Chunked, Out::Length)
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        code,
        parts.status.canonical_reason().unwrap_or("Unknown")
    )
    .into_bytes();
    for (name, value) in &parts.headers {
        if !is_hop(name, &named) && name != "content-length" {
            head.extend_from_slice(name.as_str().as_bytes());
            head.extend_from_slice(b": ");
            head.extend_from_slice(value.as_bytes());
            head.extend_from_slice(b"\r\n");
        }
    }
    match out {
        Out::Length(n) => head.extend_from_slice(format!("content-length: {n}\r\n").as_bytes()),
        Out::Chunked => head.extend_from_slice(b"transfer-encoding: chunked\r\n"),
        Out::Bodiless(Some(n)) => {
            head.extend_from_slice(format!("content-length: {n}\r\n").as_bytes());
        }
        Out::Bodiless(None) => {}
    }
    head.extend_from_slice(b"connection: close\r\n\r\n");
    if stream.write_all(&head).is_err() || stream.flush().is_err() {
        return Written::Complete(code);
    }
    if matches!(out, Out::Bodiless(_)) {
        return Written::Complete(code);
    }
    if copy_body(&mut body.as_reader(), stream, out).is_err() {
        abort(stream);
        return Written::Aborted;
    }
    Written::Complete(code)
}

/// Whether `name` is hop-by-hop or listed in the `connection` header.
fn is_hop(name: &HeaderName, named: &[String]) -> bool {
    HOP_BY_HOP.contains(&name.as_str()) || named.iter().any(|t| t == name.as_str())
}

/// Makes the close of `stream` send a reset instead of a clean end.
fn abort(stream: &TcpStream) {
    let _ = rustix::net::sockopt::set_socket_linger(stream, Some(Duration::ZERO));
}

/// Copies `from` to `to` through a fixed buffer, flushing after every read.
///
/// Fails when `from` errors or ends before the declared length; the closing
/// chunk is written only after a clean end. A client write failure ends the
/// copy quietly, as there is nobody left to mislead.
fn copy_body(from: &mut impl Read, to: &mut TcpStream, out: Out) -> io::Result<()> {
    let mut buf = [0_u8; COPY_BUF];
    let mut sent = 0_u64;
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            return match out {
                Out::Length(want) if sent < want => {
                    Err(io::Error::from(io::ErrorKind::UnexpectedEof))
                }
                Out::Length(_) | Out::Bodiless(_) => Ok(()),
                Out::Chunked => {
                    let _ = to.write_all(b"0\r\n\r\n").and_then(|()| to.flush());
                    Ok(())
                }
            };
        }
        let Some(chunk) = buf.get(..n) else {
            return Ok(());
        };
        sent += n as u64;
        let wrote = match out {
            Out::Length(_) | Out::Bodiless(_) => to.write_all(chunk),
            Out::Chunked => to
                .write_all(format!("{n:x}\r\n").as_bytes())
                .and_then(|()| to.write_all(chunk))
                .and_then(|()| to.write_all(b"\r\n")),
        };
        if wrote.and_then(|()| to.flush()).is_err() {
            return Ok(());
        }
    }
}
