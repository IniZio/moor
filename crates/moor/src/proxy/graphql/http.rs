//! HTTP/1.1 request and response framing for the GraphQL filter.
//!
//! The parser is strict, so no message can be read two ways, and it keeps the
//! exact wire bytes so an allowed request is forwarded verbatim.

use std::io::{self, BufRead, ErrorKind, Read, Write};

/// Largest request or response head, in bytes, including the final blank line.
pub(crate) const MAX_HEAD: usize = 64 * 1024;

/// Largest chunk-size line, in bytes, including its line ending.
const MAX_CHUNK_LINE: usize = 1024;

/// A parsed request head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Head {
    /// The exact bytes read, through the terminating blank line.
    pub(crate) raw: Vec<u8>,
    /// The request method.
    pub(crate) method: String,
    /// The request target, or the authority for `CONNECT`.
    pub(crate) target: String,
    /// The header fields in wire order.
    pub(crate) headers: Vec<(String, String)>,
}

impl Head {
    /// The value of the first header named `name`, ignoring ASCII case.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A parsed response head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResponseHead {
    /// The exact bytes read, through the terminating blank line.
    pub(crate) raw: Vec<u8>,
    /// The status code.
    pub(crate) status: u16,
    /// The header fields in wire order.
    pub(crate) headers: Vec<(String, String)>,
}

/// How a message body is delimited on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Framing {
    /// No body.
    Empty,
    /// A body of exactly this many bytes.
    Length(u64),
    /// A chunked body.
    Chunked,
    /// A body that ends when the peer closes the connection.
    UntilClose,
}

/// A body read into memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Body {
    /// The exact wire bytes, including chunk sizes, extensions and trailers.
    pub(crate) raw: Vec<u8>,
    /// The payload with the framing removed.
    pub(crate) decoded: Vec<u8>,
}

/// Why a message could not be read.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HttpError {
    /// The peer closed the connection before sending any byte.
    #[error("connection closed")]
    Closed,
    /// The message is not acceptable HTTP/1.1.
    #[error("malformed http message")]
    Malformed,
    /// The message exceeds a size limit.
    #[error("http message too large")]
    TooLarge,
    /// The transport failed.
    #[error("i/o error: {0}")]
    Io(#[source] io::Error),
}

/// Maps a transport error, treating a truncated stream as a malformed message.
fn io_err(e: io::Error) -> HttpError {
    if e.kind() == ErrorKind::UnexpectedEof {
        HttpError::Malformed
    } else {
        HttpError::Io(e)
    }
}

/// Reads one CRLF-terminated line onto `buf`, failing once `buf` would exceed
/// `limit` bytes, and returns its length without the line ending. Returns
/// `None` on end of input before any byte.
fn read_crlf_line(
    r: &mut impl BufRead,
    buf: &mut Vec<u8>,
    limit: usize,
) -> Result<Option<usize>, HttpError> {
    let start = buf.len();
    let room = u64::try_from(limit.saturating_sub(start)).map_or(u64::MAX, |n| n.saturating_add(1));
    r.by_ref()
        .take(room)
        .read_until(b'\n', buf)
        .map_err(HttpError::Io)?;
    if buf.len() > limit {
        return Err(HttpError::TooLarge);
    }
    match buf.len() - start {
        0 => Ok(None),
        n if buf.ends_with(b"\r\n") => Ok(Some(n - 2)),
        _ => Err(HttpError::Malformed),
    }
}

/// Whether `s` is a non-empty RFC 7230 token.
fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// The raw bytes, start line and headers of a head.
type RawHead = (Vec<u8>, String, Vec<(String, String)>);

/// Reads a head.
fn read_raw_head(r: &mut impl BufRead) -> Result<RawHead, HttpError> {
    let mut raw = Vec::new();
    loop {
        let start = raw.len();
        match read_crlf_line(r, &mut raw, MAX_HEAD)? {
            None if start == 0 => return Err(HttpError::Closed),
            None => return Err(HttpError::Malformed),
            Some(0) if start == 0 => return Err(HttpError::Malformed),
            Some(0) => break,
            Some(_) => {}
        }
    }
    let text = std::str::from_utf8(&raw).map_err(|_| HttpError::Malformed)?;
    let mut lines = text.split("\r\n");
    let first = lines.next().ok_or(HttpError::Malformed)?.to_owned();
    let mut headers = Vec::new();
    for line in lines.take_while(|l| !l.is_empty()) {
        if line.contains(['\r', '\n', '\0']) {
            return Err(HttpError::Malformed);
        }
        let (name, value) = line.split_once(':').ok_or(HttpError::Malformed)?;
        if !is_token(name) {
            return Err(HttpError::Malformed);
        }
        headers.push((name.to_owned(), value.trim_matches([' ', '\t']).to_owned()));
    }
    if first.contains(['\r', '\n', '\0']) {
        return Err(HttpError::Malformed);
    }
    Ok((raw, first, headers))
}

/// Reads a request head, including `CONNECT` heads.
pub(crate) fn read_head(r: &mut impl BufRead) -> Result<Head, HttpError> {
    let (raw, first, headers) = read_raw_head(r)?;
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(HttpError::Malformed);
    };
    let target_ok = !target.is_empty() && !target.bytes().any(|b| b.is_ascii_control());
    if !is_token(method) || !target_ok || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(HttpError::Malformed);
    }
    Ok(Head {
        method: method.to_owned(),
        target: target.to_owned(),
        raw,
        headers,
    })
}

/// Reads a response head.
pub(crate) fn read_response_head(r: &mut impl BufRead) -> Result<ResponseHead, HttpError> {
    let (raw, first, headers) = read_raw_head(r)?;
    let mut parts = first.splitn(3, ' ');
    let (Some(version), Some(code)) = (parts.next(), parts.next()) else {
        return Err(HttpError::Malformed);
    };
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0")
        || code.len() != 3
        || !code.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(HttpError::Malformed);
    }
    let status = code.parse().map_err(|_| HttpError::Malformed)?;
    Ok(ResponseHead {
        raw,
        status,
        headers,
    })
}

/// The framing a header block declares, or `None` when it declares none.
fn declared_framing(headers: &[(String, String)]) -> Result<Option<Framing>, HttpError> {
    let mut codings = headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("transfer-encoding"))
        .flat_map(|(_, v)| v.split(','))
        .map(str::trim)
        .peekable();
    let mut lengths = headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.as_str())
        .peekable();
    if codings.peek().is_some() {
        let only_chunked = codings.all(|c| c.eq_ignore_ascii_case("chunked"));
        if !only_chunked || lengths.peek().is_some() {
            return Err(HttpError::Malformed);
        }
        return Ok(Some(Framing::Chunked));
    }
    let Some(first) = lengths.next() else {
        return Ok(None);
    };
    if lengths.any(|v| v != first) || first.is_empty() || !first.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(HttpError::Malformed);
    }
    first
        .parse()
        .map(|n| Some(Framing::Length(n)))
        .map_err(|_| HttpError::Malformed)
}

/// The framing of a request body. Requests never run until close.
pub(crate) fn request_framing(h: &Head) -> Result<Framing, HttpError> {
    Ok(declared_framing(&h.headers)?.unwrap_or(Framing::Empty))
}

/// The framing of a response body to a request with method `request_method`.
pub(crate) fn response_framing(
    h: &ResponseHead,
    request_method: &str,
) -> Result<Framing, HttpError> {
    let bodyless = request_method == "HEAD"
        || (request_method == "CONNECT" && (200..300).contains(&h.status))
        || matches!(h.status, 100..=199 | 204 | 304);
    if bodyless {
        return Ok(Framing::Empty);
    }
    Ok(declared_framing(&h.headers)?.unwrap_or(Framing::UntilClose))
}

/// Where the bytes of a body go while it is walked.
struct Sink<'a> {
    /// Receives every wire byte.
    raw: &'a mut dyn Write,
    /// Receives payload bytes and the largest total payload allowed.
    decoded: Option<(&'a mut Vec<u8>, usize)>,
}

impl Sink<'_> {
    /// Fails when `more` further payload bytes would exceed the cap.
    fn check(&self, more: u64) -> Result<(), HttpError> {
        if let Some((d, cap)) = &self.decoded {
            let room = cap.saturating_sub(d.len());
            if usize::try_from(more).map_or(true, |m| m > room) {
                return Err(HttpError::TooLarge);
            }
        }
        Ok(())
    }

    /// Passes `n` payload bytes from `r` through.
    fn data(&mut self, r: &mut impl BufRead, n: u64) -> Result<(), HttpError> {
        self.check(n)?;
        let mut body = r.by_ref().take(n);
        let got = if let Some((d, _)) = &mut self.decoded {
            let start = d.len();
            body.read_to_end(d).map_err(HttpError::Io)?;
            let fresh = d.get(start..).unwrap_or_default();
            self.raw.write_all(fresh).map_err(HttpError::Io)?;
            fresh.len() as u64
        } else {
            io::copy(&mut body, self.raw).map_err(HttpError::Io)?
        };
        (got == n).then_some(()).ok_or(HttpError::Malformed)
    }

    /// Passes a CRLF-terminated line of at most `limit` bytes through and
    /// returns it without the line ending.
    fn line(&mut self, r: &mut impl BufRead, limit: usize) -> Result<Vec<u8>, HttpError> {
        let mut buf = Vec::new();
        let len = read_crlf_line(r, &mut buf, limit)?.ok_or(HttpError::Malformed)?;
        self.raw.write_all(&buf).map_err(HttpError::Io)?;
        buf.truncate(len);
        Ok(buf)
    }

    /// Walks a chunked body, including extensions and trailers.
    fn chunked(&mut self, r: &mut impl BufRead) -> Result<(), HttpError> {
        loop {
            let line = self.line(r, MAX_CHUNK_LINE)?;
            let line = std::str::from_utf8(&line).map_err(|_| HttpError::Malformed)?;
            let size_text = line.split(';').next().unwrap_or_default();
            if size_text.is_empty() || !size_text.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(HttpError::Malformed);
            }
            let size = u64::from_str_radix(size_text, 16).map_err(|_| HttpError::Malformed)?;
            if size == 0 {
                break;
            }
            self.data(r, size)?;
            let mut end = [0_u8; 2];
            r.read_exact(&mut end).map_err(io_err)?;
            if &end != b"\r\n" {
                return Err(HttpError::Malformed);
            }
            self.raw.write_all(&end).map_err(HttpError::Io)?;
        }
        let mut used = 0;
        loop {
            let trailer = self.line(r, MAX_HEAD.saturating_sub(used))?;
            if trailer.is_empty() {
                return Ok(());
            }
            used += trailer.len() + 2;
        }
    }

    /// Walks a body with the given framing.
    fn walk(&mut self, r: &mut impl BufRead, f: &Framing) -> Result<(), HttpError> {
        match f {
            Framing::Empty => Ok(()),
            Framing::Length(n) => self.data(r, *n),
            Framing::Chunked => self.chunked(r),
            Framing::UntilClose => io::copy(r, self.raw).map(drop).map_err(HttpError::Io),
        }
    }
}

/// Reads a whole body, failing with `TooLarge` once its payload passes `cap`.
pub(crate) fn read_body(r: &mut impl BufRead, f: &Framing, cap: usize) -> Result<Body, HttpError> {
    if *f == Framing::UntilClose {
        return Err(HttpError::Malformed);
    }
    let mut raw = Vec::new();
    let mut decoded = Vec::new();
    Sink {
        raw: &mut raw,
        decoded: Some((&mut decoded, cap)),
    }
    .walk(r, f)?;
    Ok(Body { raw, decoded })
}

/// Streams a body from `r` to `w` byte for byte, in bounded memory.
pub(crate) fn copy_body(
    r: &mut impl BufRead,
    w: &mut impl Write,
    f: &Framing,
) -> Result<(), HttpError> {
    Sink {
        raw: w,
        decoded: None,
    }
    .walk(r, f)
}

/// Whether a `Connection` header carries the `close` token.
pub(crate) fn wants_close(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, v)| v.split(','))
        .any(|t| t.trim().eq_ignore_ascii_case("close"))
}

/// Writes a JSON error response and announces that the connection closes.
pub(crate) fn write_refusal(w: &mut impl Write, status: u16, message: &str) -> io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        403 => "Forbidden",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let body = serde_json::json!({ "message": message }).to_string();
    let msg = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    w.write_all(msg.as_bytes())?;
    w.flush()
}

#[cfg(test)]
mod tests;
