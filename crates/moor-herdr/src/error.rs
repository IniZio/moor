//! Why a `moor-herdr` command failed.

use moor_client::ClientError;
use moor_core::user::SecretName;
use moor_core::view::user::StartNeedsValue;

/// The names of the secrets a refused start lacks, comma separated.
fn missing(needs: &StartNeedsValue) -> String {
    let names: Vec<&str> = needs.missing.iter().map(SecretName::as_str).collect();
    names.join(", ")
}

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
    /// The controller refused a close that would discard work, or could not check for it; a
    /// forced close goes ahead.
    #[error("{0}; [discard & close] closes it anyway")]
    WouldDiscard(String),
    /// The controller refused a start for secrets the person has not stored.
    #[error("controller: log in first: missing {}", missing(.0))]
    LoginRequired(Box<StartNeedsValue>),
    /// The controller's answer had an unexpected shape.
    #[error("unreadable controller answer: {0}")]
    Json(#[from] serde_json::Error),
    /// The terminal could not be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
