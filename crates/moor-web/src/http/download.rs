//! Downloads: a response body of known length, written while it is produced.

use std::io::{self, Write};

use super::{respond, write_head, HttpResponse};

/// Writes a download body in pieces through the sink it is given.
///
/// Its error answer is sent only when no piece has gone out; otherwise the connection drops
/// and the client sees a body shorter than its `Content-Length`.
pub type Fill =
    Box<dyn FnOnce(&mut dyn FnMut(&[u8]) -> io::Result<()>) -> Result<(), HttpResponse> + Send>;

/// A response whose body of known length is written while it is produced; the head goes out
/// with the first piece of the body.
pub struct Download {
    /// Status line text, such as `200 OK`.
    pub status: &'static str,
    /// `Content-Type` value.
    pub content_type: String,
    /// Extra headers.
    pub headers: Vec<(&'static str, String)>,
    /// `Content-Length`; a successful `fill` writes exactly this many bytes.
    pub len: u64,
    /// Writes the body.
    pub fill: Fill,
}

/// Writes `d`, its head going out with the first piece of the body. Returns the fill's error
/// answer when it failed before any piece, for the caller to send instead.
///
/// # Errors
/// Fails when a write fails, or when the fill failed after the head went out; the caller then
/// drops the connection.
pub(super) fn send_download(out: &mut impl Write, d: Download) -> io::Result<Option<HttpResponse>> {
    let Download {
        status,
        content_type,
        headers,
        len,
        fill,
    } = d;
    let extra: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut started = false;
    let filled = fill(&mut |piece| {
        if !started {
            write_head(out, status, &content_type, len, &extra)?;
            started = true;
        }
        out.write_all(piece)
    });
    match filled {
        Ok(()) if started => out.flush().map(|()| None),
        Ok(()) => respond(out, status, &content_type, &extra, b"").map(|()| None),
        Err(failed) if !started => Ok(Some(failed)),
        Err(_) => Err(io::Error::other("download failed after its head was sent")),
    }
}
