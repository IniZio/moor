//! What a sleep decided about saving the Box's caches, as its `asleep` event records it.

use serde::{Deserialize, Serialize};

/// Whether a sleep let the provider save the Box's caches.
///
/// This records what moor asked of the provider, not what the provider then did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CacheSave {
    /// The provider was asked to save them.
    Allowed,
    /// The provider was asked to save nothing.
    Denied {
        /// Why.
        reason: CacheSaveDenied,
    },
}

/// Why a sleep did not let the provider save the Box's caches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSaveDenied {
    /// The agent was not quiet, so the caches could hold torn files.
    AgentNotQuiet,
    /// A run was in flight, and could be writing to the caches.
    RunInFlight,
    /// The agent could not be read, and may have started a turn since it went quiet.
    AgentUnread,
    /// The Box was suspended as a last resort, whatever it was doing.
    LastResort,
    /// The provider or the machine's agent does not report the feature.
    NoFeature,
    /// The Box's machine is not linked and online as its owner's, so nothing says what it reports.
    MachineOffline,
}

#[cfg(test)]
mod tests;
