//! Minimal HTTP/1.1 request parsing and response writing for the web UI.

use std::io::{self, Read, Write};

/// Largest request head accepted, in bytes.
const MAX_HEAD: usize = 16 * 1024;

/// Largest request body accepted, in bytes.
const MAX_BODY: usize = 64 * 1024;

/// Headers added to every response.
const COMMON_HEADERS: &str = "Connection: close\r\n\
X-Content-Type-Options: nosniff\r\n\
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'\r\n\
Referrer-Policy: no-referrer\r\n";

/// A parsed request.
pub(super) struct Request {
    /// Request method, as sent.
    pub(super) method: String,
    /// Path without the query string.
    pub(super) path: String,
    /// Raw query string, empty when absent.
    query: String,
    /// Header names (lowercased) with their values.
    headers: Vec<(String, String)>,
    /// Body bytes; empty when the request sent no `content-length`.
    pub(super) body: Vec<u8>,
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

/// Reads and parses one request, head and body.
///
/// # Errors
/// Fails on I/O errors or timeout, an oversized head, early EOF or a malformed head.
/// Body failures are told apart by kind: `InvalidInput` means the body is over
/// `MAX_BODY` (answer 413); `InvalidData` (bad `content-length`, chunked
/// encoding) and `UnexpectedEof` (connection closed mid-body) mean 400.
pub(super) fn read_request(stream: &mut impl Read) -> io::Result<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_HEAD {
            return Err(invalid("request head too large"));
        }
        let n = stream.read(&mut chunk)?;
        let got = chunk.get(..n).unwrap_or_default();
        if got.is_empty() {
            return Err(invalid("connection closed before end of head"));
        }
        buf.extend_from_slice(got);
    };
    let mut body = buf.split_off(head_end);
    let mut req = parse_head(&String::from_utf8_lossy(&buf))?;
    if req.header("transfer-encoding").is_some() {
        return Err(invalid("transfer-encoding is not supported"));
    }
    let Some(len) = req.header("content-length") else {
        return Ok(req);
    };
    let len: usize = len.parse().map_err(|_| invalid("bad content-length"))?;
    if len > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "request body too large",
        ));
    }
    // Bytes past the declared length are not ours to keep.
    body.truncate(len);
    while body.len() < len {
        let want = (len - body.len()).min(chunk.len());
        let n = stream.read(chunk.get_mut(..want).unwrap_or_default())?;
        let got = chunk.get(..n).unwrap_or_default();
        if got.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before end of body",
            ));
        }
        body.extend_from_slice(got);
    }
    req.body = body;
    Ok(req)
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
        body: Vec::new(),
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

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "test assertions slice parsed buffers"
)]
mod tests {
    use super::*;

    /// Yields one scripted slice per `read` call.
    struct Pieces(Vec<Vec<u8>>);

    impl Read for Pieces {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Ok(0);
            }
            let piece = self.0.remove(0);
            out[..piece.len()].copy_from_slice(&piece);
            Ok(piece.len())
        }
    }

    fn read(pieces: &[&[u8]]) -> io::Result<Request> {
        read_request(&mut Pieces(pieces.iter().map(|p| p.to_vec()).collect()))
    }

    #[test]
    fn body_of_exact_length_arrives_with_head() {
        let req = read(&[b"POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello"]).unwrap();
        assert_eq!(req.body, b"hello");
    }

    #[test]
    fn body_split_across_reads_is_joined() {
        let req = read(&[
            b"POST /x HTTP/1.1\r\nContent-Length: 6\r\n\r\nab",
            b"cd",
            b"ef",
        ])
        .unwrap();
        assert_eq!(req.body, b"abcdef");
    }

    #[test]
    fn head_split_mid_terminator_keeps_leftover() {
        let req = read(&[b"POST /x HTTP/1.1\r\nContent-Length: 2\r\n\r", b"\nok"]).unwrap();
        assert_eq!(req.body, b"ok");
    }

    #[test]
    fn missing_content_length_gives_empty_body() {
        let req = read(&[b"GET /x HTTP/1.1\r\nHost: h\r\n\r\n"]).unwrap();
        assert!(req.body.is_empty());
    }

    #[test]
    fn oversize_body_is_invalid_input() {
        let head = format!(
            "POST /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        let err = read(&[head.as_bytes()]).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn chunked_is_invalid_data() {
        let err = read(&[b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"])
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn unparsable_length_is_invalid_data() {
        let err = read(&[b"POST /x HTTP/1.1\r\nContent-Length: abc\r\n\r\n"])
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn short_body_is_unexpected_eof() {
        let err = read(&[b"POST /x HTTP/1.1\r\nContent-Length: 9\r\n\r\nabc"])
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn csp_forbids_forms_framing_and_base() {
        for d in [
            "form-action 'none'",
            "frame-ancestors 'none'",
            "base-uri 'none'",
        ] {
            assert!(COMMON_HEADERS.contains(d), "{d}");
        }
    }
}
