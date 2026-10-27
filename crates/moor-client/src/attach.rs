//! The `attach` request: the agent's terminal both ways over one connection.

use std::io::{self, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

use moor_core::controller::{ClientFrame, BYTES_MAX};
use moor_core::request::{AttachReq, Request};

use super::{ByteStream, Client, ClientError, REQUEST_ID};

/// The caller's half of a running `attach`: keystrokes and terminal sizes for the agent.
#[derive(Debug)]
pub struct AttachKeys {
    /// Write half of the relay connection.
    writer: UnixStream,
}

impl AttachKeys {
    /// Sends keystrokes, as bytes frames of at most [`BYTES_MAX`] bytes each.
    ///
    /// # Errors
    /// Returns the write error; the relay is then over.
    pub fn send(&mut self, keys: &[u8]) -> io::Result<()> {
        for part in keys.chunks(BYTES_MAX) {
            let head = ClientFrame::Bytes {
                id: REQUEST_ID.to_owned(),
                len: part.len(),
            };
            self.writer.write_all(head.to_line().as_bytes())?;
            self.writer.write_all(part)?;
        }
        self.writer.flush()
    }

    /// Tells the agent's terminal its new size.
    ///
    /// # Errors
    /// Returns the write error; the relay is then over.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let frame = ClientFrame::Resize {
            id: REQUEST_ID.to_owned(),
            cols,
            rows,
        };
        self.writer.write_all(frame.to_line().as_bytes())?;
        self.writer.flush()
    }

    /// Ends the relay from this side. The screen half then reads on to the response, whose
    /// value says the client ended it.
    ///
    /// # Errors
    /// Returns the shutdown error.
    pub fn detach(&self) -> io::Result<()> {
        self.writer.shutdown(Shutdown::Write)
    }
}

impl Client {
    /// Relays the agent's terminal of the Box `req` names: the screen comes back as a
    /// [`ByteStream`] that ends with the controller's response, and keystrokes go through
    /// [`AttachKeys`]. Only the dial and the request write are retried. The connection waits
    /// for the screen without a read timeout, since an idle agent prints nothing.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the connection, handshake or request write fails.
    pub fn attach(&self, req: AttachReq) -> Result<(ByteStream, AttachKeys), ClientError> {
        let request = Request::Attach(req);
        let conn = self.redialing(|| self.sent(&request))?;
        conn.writer.set_read_timeout(None)?;
        let writer = conn.writer.try_clone()?;
        Ok((ByteStream::new(conn), AttachKeys { writer }))
    }
}
