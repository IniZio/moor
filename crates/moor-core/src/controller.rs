//! Wire frames for the controller socket: newline-delimited JSON, one frame per line.
//!
//! A bytes frame is the one exception: its JSON line is followed by `len` raw bytes.

use std::io::{self, Write};

use serde::{Deserialize, Serialize};

use crate::name::{BoxId, BoxName, SessionId};
use crate::request::Request;
use crate::response::Response;
use crate::user::Identity;

/// Protocol revision a client announces in its hello frame.
pub const PROTOCOL: u32 = 12;

/// Most raw bytes one bytes frame carries; a reader refuses a larger `len`.
pub const BYTES_MAX: usize = 64 * 1024;

/// A frame sent by a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ClientFrame {
    /// Opens the session; must be the first frame.
    Hello {
        /// Protocol revision the client speaks.
        protocol: u32,
    },
    /// Runs one request.
    Request {
        /// Client-chosen id echoed on the matching response.
        id: String,
        /// The command to run.
        request: Box<Request>,
    },
    /// Streams stored events whose cursor is greater than `since`; 0 means from the start.
    ///
    /// Filtered-out events are skipped but still advance the stream's position, so resuming
    /// from the last delivered cursor with the same filter leaves no gap.
    Watch {
        /// Last cursor the client has seen.
        since: i64,
        /// Only these Boxes; all when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        boxes: Option<Vec<BoxName>>,
        /// Only these event kinds; all when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kinds: Option<Vec<String>>,
        /// Keep streaming new events; false sends a caught-up frame, then closes the stream.
        #[serde(default = "follow_default", skip_serializing_if = "is_true")]
        follow: bool,
        /// The identity whose Boxes the stream carries, checked against the socket the watch
        /// arrived on as a request's would be; the controller socket's own peer when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity: Option<Identity>,
    },
}

/// Serde default: a watch follows unless told otherwise.
const fn follow_default() -> bool {
    true
}

/// Serde skip predicate: omit `follow` when it is the default.
#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
const fn is_true(value: &bool) -> bool {
    *value
}

/// Which events a watch delivers; `None` matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchFilter {
    /// Only these Boxes.
    pub boxes: Option<Vec<BoxName>>,
    /// Only these event kinds.
    pub kinds: Option<Vec<String>>,
}

impl WatchFilter {
    /// Whether an event of Box `box_name` (none for a Box-less event) and kind `kind` passes.
    #[must_use]
    pub fn matches(&self, box_name: Option<&str>, kind: &str) -> bool {
        self.boxes.as_ref().is_none_or(|boxes| {
            box_name.is_some_and(|name| boxes.iter().any(|b| b.as_str() == name))
        }) && self
            .kinds
            .as_ref()
            .is_none_or(|kinds| kinds.iter().any(|k| k == kind))
    }
}

/// A frame sent by the controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "kebab-case")]
pub enum ServerFrame {
    /// Answers the client hello.
    Hello {
        /// Protocol revision the controller speaks.
        protocol: u32,
    },
    /// A progress note for a request still running; zero or more precede its response.
    Progress {
        /// Id of the request this note belongs to.
        id: String,
        /// Human-readable progress text.
        message: String,
    },
    /// Raw bytes for a request still running; exactly `len` bytes follow the line.
    ///
    /// `artifact-read` sends the Artifact's content this way, and `run` sends the command's
    /// output as it reaches the run log.
    Bytes {
        /// Id of the request the bytes belong to.
        id: String,
        /// Number of raw bytes after the line, at most [`BYTES_MAX`].
        len: usize,
    },
    /// Result of one request.
    Response {
        /// Id of the request this answers.
        id: String,
        /// The CLI JSON envelope.
        envelope: Response,
    },
    /// A frame-level failure, not a command failure.
    Error {
        /// Id of the offending request, when it could be recovered.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// What went wrong.
        kind: FrameErrorKind,
        /// Human-readable detail.
        message: String,
    },
    /// One stored event, sent to a watching client.
    Event {
        /// The event's store id; resume a watch from the last one seen.
        cursor: i64,
        /// Name of the Box the event belongs to; absent for events without a Box.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#box: Option<String>,
        /// Id of the Box the event belongs to; absent for events without a Box.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        box_id: Option<BoxId>,
        /// Id of the Session the event belongs to, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
        /// Event kind.
        kind: String,
        /// Kind-specific data.
        payload: serde_json::Value,
        /// Event timestamp.
        ts: i64,
    },
    /// Sent once before the controller closes a watch with `follow` off; `cursor` is the last event id scanned, including events the filter skipped — resume from it.
    CaughtUp {
        /// Last event id scanned.
        cursor: i64,
    },
}

/// Why the controller rejected a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FrameErrorKind {
    /// The line is not a valid client frame.
    Malformed,
    /// The client hello names a protocol the controller does not speak.
    UnsupportedProtocol,
    /// A request arrived before the hello.
    HelloRequired,
    /// The client did not read fast enough and the bounded buffer filled; the controller
    /// closed the stream, so reconnect with the last cursor.
    SlowClient,
    /// The controller could not read its event store; the stream closed, so retry with
    /// the last cursor.
    StoreUnavailable,
    /// The controller is shutting down. With an `id`, that wait did not finish; without one,
    /// the connection is ending and no request on it that got no response ran. A watch
    /// closed. Send it again once a controller listens.
    Restarting,
}

/// Parses one line.
///
/// On failure returns a ready [`ServerFrame::Error`] of kind
/// [`FrameErrorKind::Malformed`], carrying `id` when the line is a JSON object with
/// a string `id` field so a client can match the error to its request.
///
/// # Errors
/// Returns the error frame to send back when the line is not a valid [`ClientFrame`].
#[allow(
    clippy::result_large_err,
    reason = "the error is the ready-to-send frame; callers write it straight to the socket"
)]
pub fn parse_frame(line: &str) -> Result<ClientFrame, ServerFrame> {
    serde_json::from_str::<ClientFrame>(line).map_err(|e| ServerFrame::Error {
        id: recover_id(line),
        kind: FrameErrorKind::Malformed,
        message: e.to_string(),
    })
}

/// Parses one controller line.
///
/// # Errors
/// Returns the JSON error when the line is not a valid [`ServerFrame`].
pub fn parse_server_frame(line: &str) -> Result<ServerFrame, serde_json::Error> {
    serde_json::from_str(line)
}

/// Reads a string `id` from a JSON object line, if present.
fn recover_id(line: &str) -> Option<String> {
    match serde_json::from_str::<serde_json::Value>(line).ok()? {
        serde_json::Value::Object(mut map) => match map.remove("id") {
            Some(serde_json::Value::String(id)) => Some(id),
            _ => None,
        },
        _ => None,
    }
}

impl ClientFrame {
    /// Renders the frame as one JSON object followed by a newline.
    #[must_use]
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).map_or_else(
            |_| String::from("\n"),
            |mut s| {
                s.push('\n');
                s
            },
        )
    }
}

impl ServerFrame {
    /// Renders the frame as one JSON object followed by a newline.
    #[must_use]
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).map_or_else(
            |_| fallback(),
            |mut s| {
                s.push('\n');
                s
            },
        )
    }
}

/// Writes `data` for request `id` as bytes frames of at most [`BYTES_MAX`] bytes each.
///
/// # Errors
/// Returns the write error; the frame stream is then broken and the connection must close.
pub fn write_bytes(out: &mut dyn Write, id: &str, data: &[u8]) -> io::Result<()> {
    for part in data.chunks(BYTES_MAX) {
        let head = ServerFrame::Bytes {
            id: id.to_owned(),
            len: part.len(),
        };
        out.write_all(head.to_line().as_bytes())?;
        out.write_all(part)?;
    }
    out.flush()
}

/// Minimal error frame used when serialisation fails.
fn fallback() -> String {
    let frame = serde_json::json!({
        "frame": "error",
        "kind": "malformed",
        "message": "failed to serialise server frame",
    });
    format!("{frame}\n")
}

#[cfg(test)]
mod tests;
