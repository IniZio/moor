//! Arguments of the `repo-config-*` requests: a repo's config, kept in the store.

use serde::{Deserialize, Serialize};

use crate::name::ForgeRepo;
use crate::user::Identity;

/// Arguments of `repo-config-get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoConfigGetReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The repo whose config to read.
    pub repo: ForgeRepo,
}

/// Arguments of `repo-config-set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoConfigSetReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The repo whose config to replace.
    pub repo: ForgeRepo,
    /// The whole config as YAML, in the repo config format.
    pub text: String,
    /// The revision the writer last saw, 0 for none; the write is refused when the stored
    /// revision differs. Absent writes whatever is stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

#[cfg(test)]
mod tests;
