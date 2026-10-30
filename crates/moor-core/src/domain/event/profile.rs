//! Events about the requester's agent profile.

use serde::Serialize;

/// The profile step that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStep {
    /// Resolving the ref to a commit.
    Resolve,
    /// Reading or parsing `moor-profile.yaml` or a marketplace.
    Manifest,
    /// Fetching the profile into the Box.
    Fetch,
    /// Installing a piece of the profile in the Box at launch.
    Install,
}

/// The profile resolved to a commit, or a step of getting it into the Box failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileEvent {
    /// The requester's agent profile resolved to a commit.
    ProfileResolved {
        /// The profile repo, `owner/name`.
        repo: crate::name::ForgeRepo,
        /// The full commit sha.
        sha: String,
    },
    /// A profile step failed; the Box starts without the profile.
    ProfileFailed {
        /// Which step failed.
        step: ProfileStep,
        /// What went wrong.
        detail: String,
    },
}

impl ProfileEvent {
    /// The `snake_case` kind name; equals the serde tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ProfileResolved { .. } => "profile_resolved",
            Self::ProfileFailed { .. } => "profile_failed",
        }
    }
}
