//! Wire frames for the controller socket: newline-delimited JSON, one frame per line.

use serde::{Deserialize, Serialize};

use crate::request::Request;
use crate::response::Response;

/// Protocol revision a client announces in its hello frame.
pub const PROTOCOL: u32 = 1;

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
    /// Streams every stored event whose cursor is greater than `since`; 0 means from the start.
    Watch {
        /// Last cursor the client has seen.
        since: i64,
    },
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
        /// Name of the Task the event belongs to.
        task: String,
        /// Event kind.
        kind: String,
        /// Kind-specific data.
        payload: serde_json::Value,
        /// Event timestamp.
        ts: i64,
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
