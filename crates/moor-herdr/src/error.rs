//! Why a `moor-herdr` command failed.

use moor_client::ClientError;

/// A failed `moor-herdr` step.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A herdr CLI call failed or answered something unreadable.
    #[error("herdr {args}: {message}")]
    Herdr {
        /// The arguments of the call.
        args: String,
        /// What went wrong.
        message: String,
    },
    /// The command line asks for something this front cannot do.
    #[error("{0}")]
    Usage(String),
    /// The controller could not be reached or the exchange broke.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The controller answered with an error.
    #[error("controller: {0}")]
    Refused(String),
    /// The controller's answer had an unexpected shape.
    #[error("unreadable controller answer: {0}")]
    Json(#[from] serde_json::Error),
    /// The terminal could not be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
