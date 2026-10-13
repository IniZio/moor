//! Bounded HTTP/1.x request-head reader for the loopback proxy listener.
//!
//! Memory use is capped at [`MAX_HEAD_BYTES`] plus one read chunk, so a
//! hostile local client cannot exhaust the proxy with an endless head.

use std::io::{ErrorKind, Read};

/// Largest request head, in bytes, that is buffered before rejecting.
pub(crate) const MAX_HEAD_BYTES: usize = 16 * 1024;
/// Largest number of header fields accepted in one head.
pub(crate) const MAX_HEADERS: usize = 64;
/// Size of each read from the peer.
const CHUNK_BYTES: usize = 1024;

/// Parsed request line and header fields.
#[derive(Debug)]
pub(crate) struct Head {
    /// Request method, for example `GET` or `CONNECT`.
    pub(crate) method: String,
    /// Request target exactly as sent.
    pub(crate) target: String,
    /// Header fields in wire order, name and value as sent.
    pub(crate) headers: Vec<(String, String)>,
}

impl Head {
    /// True when an `Expect: 100-continue` header is present (name and value
    /// compared case-insensitively).
    pub(crate) fn expects_continue(&self) -> bool {
        self.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("expect") && value.trim().eq_ignore_ascii_case("100-continue")
        })
    }

    /// Borrowed view of the header fields.
    pub(crate) fn header_pairs(&self) -> Vec<(&str, &str)> {
        self.headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect()
    }
}

/// Why a request head could not be read.
#[derive(Debug)]
pub(crate) enum HeadError {
    /// No complete head within [`MAX_HEAD_BYTES`].
    TooLarge,
    /// More than [`MAX_HEADERS`] header fields.
    TooManyHeaders,
    /// Syntax error, non-UTF-8 text, or EOF in the middle of a head.
    Malformed,
    /// Transport failure other than a reset before any byte arrived.
    Io(std::io::Error),
}

impl HeadError {
    /// HTTP status to answer with: 431 for oversize heads, 400 for malformed
    /// ones, `None` when the transport failed and no answer is possible.
    pub(crate) const fn status(&self) -> Option<u16> {
        match self {
            Self::TooLarge | Self::TooManyHeaders => Some(431),
            Self::Malformed => Some(400),
            Self::Io(_) => None,
        }
    }
}

/// Reads from `r` until the head ends with a blank line.
///
/// Returns `Ok(None)` when the peer closes before sending any byte, and
/// otherwise the parsed head plus any bytes read past it (the body prefix).
pub(crate) fn read_head<R: Read>(r: &mut R) -> Result<Option<(Head, Vec<u8>)>, HeadError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; CHUNK_BYTES];
    loop {
        let want = CHUNK_BYTES.min(MAX_HEAD_BYTES.saturating_sub(buf.len()));
        let Some(window) = chunk.get_mut(..want) else {
            return Err(HeadError::Malformed);
        };
        let n = match r.read(window) {
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e)
                if buf.is_empty()
                    && matches!(
                        e.kind(),
                        ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof
                    ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(HeadError::Io(e)),
        };
        if n == 0 {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Err(HeadError::Malformed)
            };
        }
        buf.extend_from_slice(window.get(..n).ok_or(HeadError::Malformed)?);
        if let Some((head, used)) = parse(&buf)? {
            let rest = buf.get(used..).ok_or(HeadError::Malformed)?.to_vec();
            return Ok(Some((head, rest)));
        }
        if buf.len() >= MAX_HEAD_BYTES {
            return Err(HeadError::TooLarge);
        }
    }
}

/// Parses `buf`; `Ok(None)` means the head is still incomplete, otherwise
/// the head and the number of bytes it occupies.
fn parse(buf: &[u8]) -> Result<Option<(Head, usize)>, HeadError> {
    let mut slots = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut slots);
    let used = match req.parse(buf) {
        Ok(httparse::Status::Complete(used)) => used,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(httparse::Error::TooManyHeaders) => return Err(HeadError::TooManyHeaders),
        Err(_) => return Err(HeadError::Malformed),
    };
    let method = req.method.ok_or(HeadError::Malformed)?.to_owned();
    let target = req.path.ok_or(HeadError::Malformed)?.to_owned();
    let mut headers = Vec::with_capacity(req.headers.len());
    for h in req.headers.iter() {
        let value = std::str::from_utf8(h.value).map_err(|_| HeadError::Malformed)?;
        headers.push((h.name.to_owned(), value.to_owned()));
    }
    Ok(Some((
        Head {
            method,
            target,
            headers,
        },
        used,
    )))
}

#[cfg(test)]
mod tests;
