//! The one error shape Driver adapters return.

use std::time::Duration;

use super::TunnelFailure;

/// The only error shape Driver adapters return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverError {
    /// No such Box.
    NotFound,
    /// Slot limit reached.
    Capacity {
        /// Slot limit, when known.
        limit: Option<u32>,
        /// Who holds the slots, when known.
        holder: Option<String>,
    },
    /// Provider quota hit.
    Quota(String),
    /// Credentials rejected.
    Auth(String),
    /// The operation outlived its allowance.
    Timeout(Duration),
    /// Provider unreachable.
    Unavailable(String),
    /// Provider cannot do this.
    Unsupported(&'static str),
    /// The provider moor looked up is not one crabbox knows.
    UnknownProvider(String),
    /// A reverse tunnel stopped or stalled before it was ready.
    Tunnel {
        /// What went wrong.
        failure: TunnelFailure,
        /// Last lines the tunnel process wrote to stderr.
        stderr_tail: String,
    },
    /// The ssh connection to the Box dropped, so whether the remote command ran is unknown.
    Transport {
        /// Last lines crabbox or ssh wrote to stderr, as one line.
        stderr_tail: String,
    },
    /// Any other failure, summarised by the adapter.
    Failed {
        /// Moor-worded summary.
        summary: String,
    },
}

impl std::error::Error for DriverError {}
