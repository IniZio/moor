//! Why a controller call failed.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use moor_core::controller::FrameErrorKind;

use super::GaveUp;

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
    /// No dial within the redial budget reached a controller that ran the request; it never
    /// ran.
    GaveUp {
        /// The budget that ran out.
        budget: Duration,
        /// The final failure.
        last: Box<Self>,
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
            Self::GaveUp { budget, last } => write!(
                f,
                "the controller did not take the request within {budget:?} of redialing; last: {last}"
            ),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect { source, .. } | Self::Io(source) => Some(source),
            Self::GaveUp { last, .. } => Some(last.as_ref()),
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

impl From<GaveUp<Self>> for ClientError {
    fn from(e: GaveUp<Self>) -> Self {
        match e {
            GaveUp::Failed(e) => e,
            GaveUp::Budget { budget, last } => Self::GaveUp {
                budget,
                last: Box::new(last),
            },
        }
    }
}
