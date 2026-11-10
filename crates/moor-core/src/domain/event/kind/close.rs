//! Events about what a close left undone.

use serde::Serialize;

use crate::response::CloseRefusal;

/// What a close left undone: the caches of a Box it deleted asleep, or the close itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CloseEvent {
    /// A close deleted the Box without waking it, so its caches were not saved.
    CachesNotSaved,
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
            Self::CloseRefused { .. } => "close_refused",
        }
    }
}
