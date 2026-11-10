//! Events about a close: its release, and what it left undone.

use serde::Serialize;

use crate::response::CloseRefusal;

/// A close's release of the Box, or what the close left undone: the caches of a Box it deleted
/// asleep, or the close itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CloseEvent {
    /// A close deleted the Box without waking it, so its caches were not saved.
    CachesNotSaved,
    /// A close asked the provider to release the Box; a provider saves writable caches now.
    ReleaseStarted,
    /// The provider answered the release.
    ReleaseFinished(ReleaseEnd),
    /// The wanted pass gave up a close moor still wanted, so the Box stays open.
    CloseRefused {
        /// Why, when the close was refused; none when it failed some other way.
        reason: Option<CloseRefusal>,
        /// The failure, in moor's words.
        error: String,
    },
}

impl CloseEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::CachesNotSaved => "caches_not_saved",
            Self::ReleaseStarted => "release_started",
            Self::ReleaseFinished(_) => "release_finished",
            Self::CloseRefused { .. } => "close_refused",
        }
    }
}

/// How a close's release ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ReleaseEnd {
    /// Time the release took, in milliseconds.
    pub ms: u64,
    /// Whether the provider released the Box.
    pub ok: bool,
    /// Whether the provider saved the Box's caches; none when it does not say.
    pub caches_saved: Option<bool>,
}
