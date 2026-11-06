//! A request whose answer is a command's stdio both ways: the caller's stdin goes up as bytes
//! frames and then an eof frame, the command's stdout and stderr come back as bytes and stderr
//! frames until the response.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use moor_core::controller::{ClientFrame, ServerFrame, BYTES_MAX};
use moor_core::request::Request;
use moor_core::response::Response;

use super::{unexpected, Client, ClientError, REQUEST_ID};

impl Client {
    /// Runs `request`, such as a `box-rsync`, with the command's stdin read from `stdin` and its
    /// output written to `stdout` and `stderr`, and returns the controller's response. Only the
    /// dial and the request write are retried. No socket timeout applies, since a transfer may
    /// sit quiet for long; the controller's own deadline ends a stuck one.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the exchange fails or `stdout` or `stderr` stops taking
    /// output.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "callers hand the request over; each dial sends a copy"
    )]
    pub fn pipe(
        &self,
        request: Request,
        stdin: impl Read + Send + 'static,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
    ) -> Result<Response, ClientError> {
        let mut conn = self.redialing(|| self.sent(&request))?;
        conn.writer.set_read_timeout(None)?;
        conn.writer.set_write_timeout(None)?;
        let writer = conn.writer.try_clone()?;
        // Detached: a read of stdin may block after the command ended.
        std::thread::spawn(move || feed(stdin, writer));
        let mut buf = vec![0; BYTES_MAX];
        loop {
            let (to, len): (&mut dyn Write, usize) = match conn.next_frame()? {
                ServerFrame::Bytes { id, len } if id == REQUEST_ID => (&mut *stdout, len),
                ServerFrame::Stderr { id, len } if id == REQUEST_ID => (&mut *stderr, len),
                ServerFrame::Response { id, envelope } if id == REQUEST_ID => return Ok(envelope),
                other => return Err(unexpected(other, "bytes, stderr or response")),
            };
            let data = buf.get_mut(..len).ok_or_else(|| {
                ClientError::Protocol(format!("bytes frame of {len} exceeds {BYTES_MAX}"))
            })?;
            conn.reader.read_exact(data)?;
            to.write_all(data).and_then(|()| to.flush())?;
        }
    }
}

/// Sends `stdin` as bytes frames, then an eof frame once it ends.
fn feed(mut stdin: impl Read, mut writer: UnixStream) {
    let mut buf = vec![0; BYTES_MAX];
    let id = REQUEST_ID.to_owned();
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let head = ClientFrame::Bytes {
            id: id.clone(),
            len: n,
        };
        let sent = writer
            .write_all(head.to_line().as_bytes())
            .and_then(|()| writer.write_all(buf.get(..n).unwrap_or_default()))
            .and_then(|()| writer.flush());
        if sent.is_err() {
            return;
        }
    }
    let _ = writer.write_all(ClientFrame::Eof { id }.to_line().as_bytes());
}
