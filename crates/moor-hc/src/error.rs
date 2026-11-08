//! Why a `moor-hc` command failed.

use moor_client::ClientError;

/// A failed `moor-hc` step.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An hc call failed or answered something unreadable.
    #[error("hc {args}: {message}")]
    Hc {
        /// The arguments of the call.
        args: String,
        /// What went wrong.
        message: String,
    },
    /// The command line asks for something moor-hc cannot do.
    #[error("{0}")]
    Usage(String),
    /// The controller could not be reached or the exchange broke.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The controller answered with an error.
    #[error("moor: {0}")]
    Refused(String),
    /// The controller's answer had an unexpected shape.
    #[error("unreadable moor answer: {0}")]
    Json(#[from] serde_json::Error),
    /// A local read or write failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
