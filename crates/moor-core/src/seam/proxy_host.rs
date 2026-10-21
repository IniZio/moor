//! Proxy host seam: starts, inspects and stops the Proxy that serves a Box.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::proxy::ResolvedRule;
use crate::name::{BoxHandle, BoxId};
use crate::seam::forge::ForgeRepo;
use crate::user::UserId;

/// What the Proxy host needs to run a Box's Proxy; it holds no secret values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxySpec {
    /// The Box the Proxy serves.
    pub box_id: BoxId,
    /// The Box, or `None` when it has none.
    pub box_handle: Option<BoxHandle>,
    /// The forge repo the Box works on, when known.
    pub box_repo: Option<ForgeRepo>,
    /// The resolved tool rules the Proxy enforces.
    #[serde(default)]
    pub rules: Vec<ResolvedRule>,
    /// The user whose secrets the Proxy injects.
    pub user: UserId,
    /// The vault key file that opens the user's secrets; the key itself never enters the spec.
    pub vault_key: PathBuf,
}

/// A running Proxy's address as the Box sees it.
pub struct ProxyEndpoint {
    /// The Proxy's port as seen inside the Box.
    pub box_port: u16,
    /// The public Box CA certificate in PEM form; not secret.
    pub ca_pem: String,
}

impl fmt::Debug for ProxyEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyEndpoint")
            .field("box_port", &self.box_port)
            .field("ca_pem_len", &self.ca_pem.len())
            .finish()
    }
}

/// Where a Box's Proxy is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyStatus {
    /// No Proxy exists for the Box.
    Absent,
    /// The Proxy was launched and is not yet ready.
    Starting,
    /// The Proxy is serving on `box_port`.
    Ready {
        /// The Proxy's port as seen inside the Box.
        box_port: u16,
    },
    /// A Proxy record exists but its process is gone.
    Stale,
}

/// A Proxy host failure; the message is the reason text shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProxyHostError {
    /// The Proxy could not be ensured or inspected.
    #[error("{0}")]
    Failed(String),
    /// A stop did not fully succeed.
    #[error("{0}")]
    Incomplete(String),
}

/// Runs Proxies for Boxes, behind whichever implementation is wired.
pub trait ProxyHost: Send + Sync {
    /// Starts the Box's Proxy if needed and returns its endpoint.
    ///
    /// # Errors
    /// Returns [`ProxyHostError::Failed`] when the Proxy cannot be made ready.
    fn ensure(&self, spec: &ProxySpec) -> Result<ProxyEndpoint, ProxyHostError>;

    /// Reports the state of the Box's Proxy.
    ///
    /// # Errors
    /// Returns [`ProxyHostError::Failed`] when the state cannot be read.
    fn status(&self, box_id: BoxId) -> Result<ProxyStatus, ProxyHostError>;

    /// Stops the Box's Proxy and forgets it.
    ///
    /// # Errors
    /// Returns [`ProxyHostError::Incomplete`] when the stop did not fully succeed.
    fn stop(&self, box_id: BoxId) -> Result<(), ProxyHostError>;
}

#[cfg(test)]
mod tests;
