//! A pull reader over the raw bytes frames of one request's answer.

use std::io::{self, Read};

use moor_core::controller::{ServerFrame, BYTES_MAX};
use moor_core::response::Response;

use super::{unexpected, ClientError, Conn, REQUEST_ID};

/// The raw bytes of one request's answer, read straight off the connection into the caller's
/// buffer. Holds no chunk of its own. Returned by [`super::Client::read_stream`].
#[derive(Debug)]
pub struct ByteStream {
    /// The open connection, positioned inside or between frames.
    conn: Conn,
    /// Bytes of the current bytes frame not yet read.
    left: usize,
    /// How the answer ended: the controller's response, or why reading failed.
    end: Option<Result<Response, ClientError>>,
}

impl ByteStream {
    /// Wraps a connection whose request was just sent.
    pub(super) const fn new(conn: Conn) -> Self {
        Self {
            conn,
            left: 0,
            end: None,
        }
    }

    /// The controller's response after the last bytes frame. An error response comes back as
    /// `Ok`, as from [`super::Client::request`].
    ///
    /// # Errors
    /// Returns [`ClientError`] when reading failed, or when bytes remain unread.
    pub fn finish(mut self) -> Result<Response, ClientError> {
        if self.end.is_none() && self.left == 0 {
            let end = match self.conn.next_frame() {
                Ok(ServerFrame::Response { id, envelope }) if id == REQUEST_ID => Ok(envelope),
                Ok(other) => Err(unexpected(other, "response")),
                Err(e) => Err(e),
            };
            self.end = Some(end);
        }
        self.end.unwrap_or_else(|| {
            Err(ClientError::Protocol(
                "bytes left unread before the response".to_owned(),
            ))
        })
    }

    /// Records `e` as the end of the stream and returns it as an I/O error for the reader.
    fn fail(&mut self, e: ClientError) -> io::Error {
        let err = io::Error::other(e.to_string());
        self.end = Some(Err(e));
        err
    }

    /// Reads frames until one carries bytes; false once the response arrived.
    fn next_bytes(&mut self) -> io::Result<bool> {
        while self.left == 0 {
            match &self.end {
                Some(Ok(reply)) => {
                    return reply
                        .error
                        .as_ref()
                        .map_or(Ok(false), |e| Err(io::Error::other(e.message.clone())));
                }
                Some(Err(e)) => return Err(io::Error::other(e.to_string())),
                None => {}
            }
            match self.conn.next_frame() {
                Ok(ServerFrame::Bytes { id, len }) if id == REQUEST_ID && len <= BYTES_MAX => {
                    self.left = len;
                }
                Ok(ServerFrame::Bytes { id, len }) if id == REQUEST_ID => {
                    let e =
                        ClientError::Protocol(format!("bytes frame of {len} exceeds {BYTES_MAX}"));
                    return Err(self.fail(e));
                }
                Ok(ServerFrame::Response { id, envelope }) if id == REQUEST_ID => {
                    self.end = Some(Ok(envelope));
                }
                Ok(other) => return Err(self.fail(unexpected(other, "bytes or response"))),
                Err(e) => return Err(self.fail(e)),
            }
        }
        Ok(true)
    }
}

impl Read for ByteStream {
    /// Copies bytes of the current frame into `buf`. Returns 0 at a successful response; an
    /// error response, a broken frame or a dropped connection is an error.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || !self.next_bytes()? {
            return Ok(0);
        }
        let want = buf.len().min(self.left);
        let n = match self
            .conn
            .reader
            .read(buf.get_mut(..want).unwrap_or_default())
        {
            Ok(0) => return Err(self.fail(io::Error::from(io::ErrorKind::UnexpectedEof).into())),
            Ok(n) => n,
            Err(e) => return Err(self.fail(e.into())),
        };
        self.left -= n;
        Ok(n)
    }
}
