//! Arguments of the `repo-config-*` requests: a repo's config, kept in the store.

use serde::{Deserialize, Serialize};

use crate::name::ForgeRepo;
use crate::user::Identity;

/// Which of a repo's texts in moor a request reads or writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoFile {
    /// The repo config: brief, setup, env and the rest.
    #[default]
    Config,
    /// The Proxy rules its Boxes follow; a repo with none stored reads the default rules text.
    Rules,
}

impl RepoFile {
    /// Whether this is the repo config, which a request leaves unsaid.
    #[must_use]
    pub fn is_config(&self) -> bool {
        *self == Self::Config
    }
}

/// Arguments of `repo-config-get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoConfigGetReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The repo whose config to read.
    pub repo: ForgeRepo,
    /// Which text to read.
    #[serde(default, skip_serializing_if = "RepoFile::is_config")]
    pub file: RepoFile,
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
    /// Which text to replace.
    #[serde(default, skip_serializing_if = "RepoFile::is_config")]
    pub file: RepoFile,
    /// The whole text as YAML, in the format of `file`.
    pub text: String,
    /// The revision the writer last saw, 0 for none; the write is refused when the stored
    /// revision differs. Absent writes whatever is stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

#[cfg(test)]
mod tests;
