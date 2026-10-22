//! Client for the moor controller socket: one-shot requests and event watches.
//!
//! A call dials again, within its [`Redial`] budget, while the request provably did not run:
//! no controller listened, the connection failed before the request was written, or the
//! controller answered `restarting`. A connection cut after the request was written is an
//! error and is never sent again, because the request may have run.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use moor_core::controller::{
    parse_server_frame, ClientFrame, FrameErrorKind, ServerFrame, WatchFilter, BYTES_MAX, PROTOCOL,
};
use moor_core::request::Request;
use moor_core::response::Response;

pub use error::ClientError;
pub use redial::{redial, Attempt, GaveUp, Redial};
pub use stream::ByteStream;
pub use watch::{EventRecord, Watch, WatchItem, WatchStop};

mod error;
mod redial;
mod stream;
mod watch;

/// Id the client puts on its single request per connection.
const REQUEST_ID: &str = "1";

/// Longest accepted frame line in bytes, newline included.
pub const MAX_FRAME: usize = 1 << 20;

/// Socket timeouts for controller calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// Longest wait for the next frame; resets whenever a frame arrives.
    pub read: Duration,
    /// Longest wait for the controller to accept written bytes.
    pub write: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            read: Duration::from_secs(120),
            write: Duration::from_secs(10),
        }
    }
}

/// Dials the controller socket.
#[derive(Debug, Clone)]
pub struct Client {
    /// Path of the controller's Unix socket.
    socket: PathBuf,
    /// Socket timeouts applied to each call.
    timeouts: Timeouts,
    /// When a call whose request did not run dials again.
    redial: Redial,
}

impl Client {
    /// A client for the controller listening on `socket`, with the default [`Redial`].
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            timeouts: Timeouts::default(),
            redial: Redial::default(),
        }
    }

    /// Replaces the default socket timeouts.
    #[must_use]
    pub const fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Replaces the default redial budget and pauses.
    #[must_use]
    pub const fn with_redial(mut self, redial: Redial) -> Self {
        self.redial = redial;
        self
    }

    /// Runs one request, passing each progress note to `on_progress`.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the connection, handshake or exchange fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "callers hand the request over; each dial sends a copy"
    )]
    pub fn request(
        &self,
        request: Request,
        mut on_progress: impl FnMut(&str),
    ) -> Result<Response, ClientError> {
        self.redialing(|| {
            let mut conn = match self.sent(&request) {
                Attempt::Done(conn) => conn,
                Attempt::Unrun(e) => return Attempt::Unrun(e),
                Attempt::Failed(e) => return Attempt::Failed(e),
            };
            loop {
                match conn.next_frame() {
                    Ok(ServerFrame::Progress { id, message }) if id == REQUEST_ID => {
                        on_progress(&message);
                    }
                    Ok(ServerFrame::Response { id, envelope }) if id == REQUEST_ID => {
                        return Attempt::Done(envelope)
                    }
                    Ok(other) => return refused(unexpected(other, "progress or response")),
                    Err(e) => return Attempt::Failed(e),
                }
            }
        })
    }

    /// Runs one request whose answer streams raw bytes, passing each bytes frame to `on_bytes`
    /// in order. Holds one frame at a time. Once a byte was passed on, the request is never
    /// sent again.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the exchange fails, a frame claims more than [`BYTES_MAX`]
    /// bytes, or `on_bytes` fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "callers hand the request over; each dial sends a copy"
    )]
    pub fn read(
        &self,
        request: Request,
        mut on_bytes: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<Response, ClientError> {
        let mut buf = vec![0; BYTES_MAX];
        self.redialing(|| {
            let mut conn = match self.sent(&request) {
                Attempt::Done(conn) => conn,
                Attempt::Unrun(e) => return Attempt::Unrun(e),
                Attempt::Failed(e) => return Attempt::Failed(e),
            };
            let mut passed = false;
            loop {
                match conn.next_frame() {
                    Ok(ServerFrame::Bytes { id, len }) if id == REQUEST_ID => {
                        passed = true;
                        let Some(data) = buf.get_mut(..len) else {
                            return Attempt::Failed(ClientError::Protocol(format!(
                                "bytes frame of {len} exceeds {BYTES_MAX}"
                            )));
                        };
                        if let Err(e) = conn.reader.read_exact(data).and_then(|()| on_bytes(data)) {
                            return Attempt::Failed(e.into());
                        }
                    }
                    Ok(ServerFrame::Response { id, envelope }) if id == REQUEST_ID => {
                        return Attempt::Done(envelope)
                    }
                    Ok(other) if passed => {
                        return Attempt::Failed(unexpected(other, "bytes or response"))
                    }
                    Ok(other) => return refused(unexpected(other, "bytes or response")),
                    Err(e) => return Attempt::Failed(e),
                }
            }
        })
    }

    /// Runs one request whose answer streams raw bytes, and returns a reader over those bytes.
    /// Call [`ByteStream::finish`] after the last byte for the controller's response. Only
    /// the dial and the request write are retried.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the connection, handshake or request write fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "callers hand the request over; each dial sends a copy"
    )]
    pub fn read_stream(&self, request: Request) -> Result<ByteStream, ClientError> {
        self.redialing(|| self.sent(&request)).map(ByteStream::new)
    }

    /// Streams stored events after cursor `since`; `follow` keeps the stream open. The first
    /// dial is tried once, so a front's own reconnect loop sees and logs the failure. A stream
    /// the controller ends with `restarting` redials and resumes from the last cursor it
    /// delivered.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the connection or handshake fails.
    pub fn watch(
        &self,
        since: i64,
        filter: WatchFilter,
        follow: bool,
    ) -> Result<Watch, ClientError> {
        Watch::open(self.clone(), since, filter, follow)
    }

    /// Runs `attempt` under this client's redial policy.
    fn redialing<T>(
        &self,
        attempt: impl FnMut() -> Attempt<T, ClientError>,
    ) -> Result<T, ClientError> {
        redial(&self.redial, attempt, |pause, _| {
            std::thread::sleep(pause);
            true
        })
        .map_err(ClientError::from)
    }

    /// Opens a connection and writes `request` on it. A failed write is not retried: the
    /// controller may have read the whole line.
    fn sent(&self, request: &Request) -> Attempt<Conn, ClientError> {
        let mut conn = match self.open() {
            Attempt::Done(conn) => conn,
            other => return other,
        };
        let frame = ClientFrame::Request {
            id: REQUEST_ID.to_owned(),
            request: Box::new(request.clone()),
        };
        match conn.send(&frame) {
            Ok(()) => Attempt::Done(conn),
            Err(e) => Attempt::Failed(e),
        }
    }

    /// Connects and completes the hello exchange. Nothing has been asked yet, so a refused
    /// connect, a dropped connection or a `restarting` answer leaves nothing run.
    fn open(&self) -> Attempt<Conn, ClientError> {
        let stream = match UnixStream::connect(&self.socket) {
            Ok(stream) => stream,
            Err(source) => {
                return Attempt::Unrun(ClientError::Connect {
                    socket: self.socket.clone(),
                    source,
                })
            }
        };
        let mut conn = match self.conn(stream) {
            Ok(conn) => conn,
            Err(e) => return Attempt::Failed(e),
        };
        let hello = conn
            .send(&ClientFrame::Hello { protocol: PROTOCOL })
            .and_then(|()| conn.read_frame());
        match hello {
            Ok(Some(ServerFrame::Hello { protocol })) if protocol == PROTOCOL => {
                Attempt::Done(conn)
            }
            Ok(Some(ServerFrame::Hello { protocol })) => Attempt::Failed(ClientError::Protocol(
                format!("controller speaks protocol {protocol}, this client speaks {PROTOCOL}"),
            )),
            Ok(Some(other)) => refused(unexpected(other, "hello")),
            Ok(None) => Attempt::Unrun(closed()),
            Err(e @ ClientError::Io(_)) => Attempt::Unrun(e),
            Err(e) => Attempt::Failed(e),
        }
    }

    /// Wraps a connected stream with this client's timeouts.
    fn conn(&self, stream: UnixStream) -> Result<Conn, ClientError> {
        stream.set_read_timeout(Some(self.timeouts.read))?;
        stream.set_write_timeout(Some(self.timeouts.write))?;
        Ok(Conn {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        })
    }
}

/// An open connection speaking NDJSON frames.
#[derive(Debug)]
struct Conn {
    /// Buffered read half.
    reader: BufReader<UnixStream>,
    /// Write half.
    writer: UnixStream,
}

impl Conn {
    /// Writes one frame and flushes.
    fn send(&mut self, frame: &ClientFrame) -> Result<(), ClientError> {
        self.writer.write_all(frame.to_line().as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    /// Reads the next frame; `None` on clean EOF.
    fn read_frame(&mut self) -> Result<Option<ServerFrame>, ClientError> {
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
    fn next_frame(&mut self) -> Result<ServerFrame, ClientError> {
        self.read_frame()?.ok_or_else(closed)
    }
}

/// The error for a connection the controller closed before answering.
fn closed() -> ClientError {
    ClientError::Protocol("controller closed the connection before answering".to_owned())
}

/// Error for a frame that does not fit the exchange; an error frame keeps its kind.
fn unexpected(frame: ServerFrame, wanted: &str) -> ClientError {
    match frame {
        ServerFrame::Error { kind, message, .. } => ClientError::Frame { kind, message },
        _ => ClientError::Protocol(format!("expected {wanted} frame, got another")),
    }
}

/// A `restarting` answer leaves the request unrun; any other refusal fails the call.
const fn refused<T>(e: ClientError) -> Attempt<T, ClientError> {
    if matches!(
        e,
        ClientError::Frame {
            kind: FrameErrorKind::Restarting,
            ..
        }
    ) {
        Attempt::Unrun(e)
    } else {
        Attempt::Failed(e)
    }
}

#[cfg(test)]
mod tests;
