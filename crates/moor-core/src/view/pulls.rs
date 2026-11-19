//! The value of `pulls`: the pull request on each of a Box's repos whose head is its branch.

use serde::{Deserialize, Serialize};

use crate::domain::pull::Pull;
use crate::name::ForgeRepo;

/// What `pulls` answers for one Box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullsValue {
    /// The Box's branch; none for a Box with no repo.
    pub branch: Option<String>,
    /// The ref the person started the Box from; none for the default branch.
    pub base: Option<String>,
    /// One entry per repo, the first repo first.
    pub repos: Vec<RepoPull>,
}

/// The pull request on one repo of a Box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoPull {
    /// The repo.
    pub repo: ForgeRepo,
    /// Whether the Box's branch is on the forge.
    pub pushed: bool,
    /// Its pull request from the Box's branch; none when there is none yet.
    pub pull: Option<Pull>,
    /// Why the forge could not be read; then `pull` says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<String>,
}
