//! Arguments of `env`.

use serde::{Deserialize, Serialize};

use crate::name::ForgeRepo;

/// Arguments of `env`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvReq {
    /// Repo whose layer to show, read through the forge like a start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<ForgeRepo>,
    /// Commit or ref to read the repo at; the default branch when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}
