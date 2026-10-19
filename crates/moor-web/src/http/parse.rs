//! Request model and parsing.

use std::io::{self, Read};

/// Largest request head accepted, in bytes.
pub(super) const MAX_HEAD: usize = 16 * 1024;

/// Largest request body accepted, in bytes.
pub(super) const MAX_BODY: usize = 64 * 1024;

/// A parsed request.
#[derive(Debug)]
pub struct HttpRequest {
    /// Request method, as sent.
    pub method: String,
    /// Path without the query string.
    pub path: String,
    /// Raw query string, empty when absent.
    query: String,
    /// Header names (lowercased) with their values.
    headers: Vec<(String, String)>,
    /// Body bytes; empty when the request sent no `content-length`.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// Value of the header `name` (lowercase), if sent.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Value of the query parameter `key`; values are taken verbatim, not percent-decoded.
    #[must_use]
    pub fn query_param(&self, key: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    /// Value of the cookie `name`, if sent.
    #[must_use]
    pub fn cookie(&self, name: &str) -> Option<&str> {
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
pub(super) fn read_request(stream: &mut impl Read) -> io::Result<HttpRequest> {
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
fn parse_head(text: &str) -> io::Result<HttpRequest> {
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
    Ok(HttpRequest {
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
