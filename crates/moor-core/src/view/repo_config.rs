//! Typed `value` payload of `repo-config-get` and `repo-config-set`.

use serde::{Deserialize, Serialize};

use crate::name::ForgeRepo;
use crate::request::RepoFile;

/// A repo's stored config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoConfigValue {
    /// The repo.
    pub repo: ForgeRepo,
    /// Which of the repo's texts this is.
    #[serde(default, skip_serializing_if = "RepoFile::is_config")]
    pub file: RepoFile,
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
    /// What the repo's Boxes met under their Proxy rules lately; only a rules read has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<RulesSeen>,
}

/// What a repo's Boxes met under their Proxy rules in the last `days` days.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesSeen {
    /// How many days back the lists look.
    pub days: u32,
    /// Each request a rule sent on without its secret, the most Boxes first.
    pub withheld: Vec<WithheldSeen>,
    /// Each host a Box reached without the Proxy, the busiest first.
    pub dialled: Vec<DialledSeen>,
}

/// One request a rule sent on without its secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithheldSeen {
    /// The request as the rule saw it: `METHOD path`, or the method alone.
    pub request: String,
    /// The host it went to.
    pub host: String,
    /// How many Boxes reported it. A Box reports each request once, so this counts Boxes, not
    /// requests.
    pub boxes: u32,
}

/// One host a Box reached without the Proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialledSeen {
    /// The name the client asked for, or the IP address of a connection that skipped the Box's
    /// own proxy.
    pub host: String,
    /// The port it went to.
    pub port: u16,
    /// Requests through the Box's own proxy, or new connections to an address, summed over the
    /// Boxes.
    pub count: u64,
}
