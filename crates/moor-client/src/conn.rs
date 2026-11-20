//! An open controller connection speaking NDJSON frames.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

use moor_core::controller::{parse_server_frame, ClientFrame, ServerFrame};

use crate::{ClientError, MAX_FRAME};

/// An open connection speaking NDJSON frames.
#[derive(Debug)]
pub(super) struct Conn {
    /// Buffered read half.
    pub(super) reader: BufReader<UnixStream>,
    /// Write half.
    pub(super) writer: UnixStream,
}

impl Conn {
    /// Writes one frame and flushes.
    pub(super) fn send(&mut self, frame: &ClientFrame) -> Result<(), ClientError> {
        self.writer.write_all(frame.to_line().as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    /// Reads the next frame; `None` on clean EOF.
    pub(super) fn read_frame(&mut self) -> Result<Option<ServerFrame>, ClientError> {
        let mut buf = Vec::new();
        (&mut self.reader)
            .take(MAX_FRAME as u64 + 1)
            .read_until(b'\n', &mut buf)?;
        if buf.is_empty() {
            return Ok(None);
        }
        if buf.len() > MAX_FRAME {
            return Err(ClientError::Protocol(format!(
                "frame longer than {MAX_FRAME} bytes"
            )));
        }
        let line = String::from_utf8(buf)
            .map_err(|e| ClientError::Protocol(format!("frame is not UTF-8: {e}")))?;
        parse_server_frame(line.trim_end())
            .map(Some)
            .map_err(|e| ClientError::Protocol(format!("malformed frame from controller: {e}")))
    }

    /// Reads the next frame; EOF is a protocol error.
    pub(super) fn next_frame(&mut self) -> Result<ServerFrame, ClientError> {
        self.read_frame()?.ok_or_else(closed)
    }
}

/// The error for a connection the controller closed before answering.
pub(super) fn closed() -> ClientError {
    ClientError::Protocol("controller closed the connection before answering".to_owned())
}
