//! Client for the moor controller socket: one-shot requests and event watches.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use moor_core::controller::{
    parse_server_frame, ClientFrame, FrameErrorKind, ServerFrame, WatchFilter, BYTES_MAX, PROTOCOL,
};
use moor_core::request::Request;
use moor_core::response::Response;
use moor_core::SessionId;

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

/// Why a controller call failed.
#[derive(Debug)]
pub enum ClientError {
    /// The socket could not be opened.
    Connect {
        /// Socket path dialed.
        socket: PathBuf,
        /// The OS error.
        source: io::Error,
    },
    /// Reading or writing the connection failed.
    Io(io::Error),
    /// The controller did not read or send within the configured timeout.
    Timeout,
    /// The controller sent an unexpected or malformed frame, or speaks another protocol.
    Protocol(String),
    /// The controller answered with an error frame.
    Frame {
        /// What the controller rejected.
        kind: FrameErrorKind,
        /// Human-readable detail.
        message: String,
    },
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect { socket, source } => {
                write!(f, "connect to {}: {source}", socket.display())
            }
            Self::Io(e) => write!(f, "controller connection: {e}"),
            Self::Timeout => f.write_str("controller connection: timed out waiting on the socket"),
            Self::Protocol(m) => write!(f, "controller protocol: {m}"),
            Self::Frame { kind, message } => write!(f, "controller error ({kind:?}): {message}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect { source, .. } | Self::Io(source) => Some(source),
            Self::Timeout | Self::Protocol(_) | Self::Frame { .. } => None,
        }
    }
}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => Self::Timeout,
            _ => Self::Io(e),
        }
    }
}

/// One stored event delivered by a watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    /// The event's store id.
    pub cursor: i64,
    /// Box the event belongs to, when it has one.
    pub box_name: Option<String>,
    /// Session the event belongs to, when known.
    pub session: Option<SessionId>,
    /// Event kind.
    pub kind: String,
    /// Kind-specific data.
    pub payload: serde_json::Value,
    /// Event timestamp.
    pub ts: i64,
}

/// One item of a watch stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchItem {
    /// A stored event.
    Event(EventRecord),
    /// The backlog is delivered; resume from `cursor`.
    CaughtUp {
        /// Last event id scanned.
        cursor: i64,
    },
}

/// Dials the controller socket.
#[derive(Debug, Clone)]
pub struct Client {
    /// Path of the controller's Unix socket.
    socket: PathBuf,
    /// Socket timeouts applied to each call.
    timeouts: Timeouts,
}

impl Client {
    /// A client for the controller listening on `socket`.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            timeouts: Timeouts::default(),
        }
    }

    /// Replaces the default socket timeouts.
    #[must_use]
    pub const fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Runs one request, passing each progress note to `on_progress`.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the connection, handshake or exchange fails.
    pub fn request(
        &self,
        request: Request,
        mut on_progress: impl FnMut(&str),
    ) -> Result<Response, ClientError> {
        let mut conn = self.open()?;
        conn.send(&ClientFrame::Request {
            id: REQUEST_ID.to_owned(),
            request: Box::new(request),
        })?;
        loop {
            match conn.next_frame()? {
                ServerFrame::Progress { id, message } if id == REQUEST_ID => on_progress(&message),
                ServerFrame::Response { id, envelope } if id == REQUEST_ID => return Ok(envelope),
                other => return Err(unexpected(other, "progress or response")),
            }
        }
    }

    /// Runs one request whose answer streams raw bytes, passing each bytes frame to `on_bytes`
    /// in order. Holds one frame at a time.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the exchange fails, a frame claims more than [`BYTES_MAX`]
    /// bytes, or `on_bytes` fails.
    pub fn read(
        &self,
        request: Request,
        mut on_bytes: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<Response, ClientError> {
        let mut conn = self.open()?;
        conn.send(&ClientFrame::Request {
            id: REQUEST_ID.to_owned(),
            request: Box::new(request),
        })?;
        let mut buf = vec![0; BYTES_MAX];
        loop {
            match conn.next_frame()? {
                ServerFrame::Bytes { id, len } if id == REQUEST_ID => {
                    let data = buf.get_mut(..len).ok_or_else(|| {
                        ClientError::Protocol(format!("bytes frame of {len} exceeds {BYTES_MAX}"))
                    })?;
                    conn.reader.read_exact(data)?;
                    on_bytes(data)?;
                }
                ServerFrame::Response { id, envelope } if id == REQUEST_ID => return Ok(envelope),
                other => return Err(unexpected(other, "bytes or response")),
            }
        }
    }

    /// Streams stored events after cursor `since`; `follow` keeps the stream open.
    ///
    /// # Errors
    /// Returns [`ClientError`] when the connection or handshake fails.
    pub fn watch(
        &self,
        since: i64,
        filter: WatchFilter,
        follow: bool,
    ) -> Result<Watch, ClientError> {
        let mut conn = self.open()?;
        conn.send(&ClientFrame::Watch {
            since,
            boxes: filter.boxes,
            kinds: filter.kinds,
            follow,
        })?;
        conn.writer.set_read_timeout(None)?;
        Ok(Watch { conn, done: false })
    }

    /// Connects and completes the hello exchange.
    fn open(&self) -> Result<Conn, ClientError> {
        let stream = UnixStream::connect(&self.socket).map_err(|source| ClientError::Connect {
            socket: self.socket.clone(),
            source,
        })?;
        stream.set_read_timeout(Some(self.timeouts.read))?;
        stream.set_write_timeout(Some(self.timeouts.write))?;
        let reader = BufReader::new(stream.try_clone()?);
        let mut conn = Conn {
            reader,
            writer: stream,
        };
        conn.send(&ClientFrame::Hello { protocol: PROTOCOL })?;
        match conn.next_frame()? {
            ServerFrame::Hello { protocol } if protocol == PROTOCOL => Ok(conn),
            ServerFrame::Hello { protocol } => Err(ClientError::Protocol(format!(
                "controller speaks protocol {protocol}, this client speaks {PROTOCOL}"
            ))),
            other => Err(unexpected(other, "hello")),
        }
    }
}

/// A live watch; iterates events until the controller closes the stream.
#[derive(Debug)]
pub struct Watch {
    /// The open connection.
    conn: Conn,
    /// Set after an error or EOF so iteration ends; caught-up frames do not stop it.
    done: bool,
}

impl Watch {
    /// A second handle on the watch socket. Calling `shutdown(Shutdown::Both)` on it from
    /// another thread makes a blocked `next()` return `None` (EOF).
    ///
    /// # Errors
    /// Returns the OS error when the socket cannot be cloned.
    pub fn shutdown_handle(&self) -> io::Result<UnixStream> {
        self.conn.writer.try_clone()
    }
}

impl Iterator for Watch {
    type Item = Result<WatchItem, ClientError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let frame = match self.conn.read_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                self.done = true;
                return None;
            }
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };
        match frame {
            ServerFrame::Event {
                cursor,
                r#box,
                session,
                kind,
                payload,
                ts,
            } => Some(Ok(WatchItem::Event(EventRecord {
                cursor,
                box_name: r#box,
                session,
                kind,
                payload,
                ts,
            }))),
            ServerFrame::CaughtUp { cursor } => Some(Ok(WatchItem::CaughtUp { cursor })),
            other => {
                self.done = true;
                Some(Err(unexpected(other, "event or caught-up")))
            }
        }
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
        self.read_frame()?.ok_or_else(|| {
            ClientError::Protocol("controller closed the connection before answering".to_owned())
        })
    }
}

/// Error for a frame that does not fit the exchange; an error frame keeps its kind.
fn unexpected(frame: ServerFrame, wanted: &str) -> ClientError {
    match frame {
        ServerFrame::Error { kind, message, .. } => ClientError::Frame { kind, message },
        _ => ClientError::Protocol(format!("expected {wanted} frame, got another")),
    }
}

#[cfg(test)]
mod tests;
