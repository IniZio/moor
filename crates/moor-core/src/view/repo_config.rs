//! Typed `value` payload of `repo-config-get` and `repo-config-set`.

use serde::{Deserialize, Serialize};

use crate::name::ForgeRepo;

/// A repo's stored config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoConfigValue {
    /// The repo.
    pub repo: ForgeRepo,
    /// The config as YAML; none when the repo has no stored config.
    pub text: Option<String>,
    /// How many times the config was written, 0 when it has not been.
    pub revision: u64,
    /// Who wrote it last: `operator`, `import`, or `user <n>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_by: Option<String>,
    /// When it was written last, RFC3339 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    /// Whether the caller may replace the config: the operator, or a user whose GitHub account
    /// can push to the repo. Only `repo-config-get` says; a reply without it reads as false.
    #[serde(default)]
    pub can_edit: bool,
}
