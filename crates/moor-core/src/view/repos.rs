//! Typed `value` payload for `repos`.

use serde::{Deserialize, Serialize};

/// One repo the controller knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoView {
    /// Name the repo is known by.
    pub name: String,
    /// Clone URL.
    pub url: String,
}

/// Value of `repos`: known repos sorted by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReposValue {
    /// The repos.
    pub repos: Vec<RepoView>,
}
