//! Arguments of `github-hook`, which the gate sends for each delivery GitHub posts to
//! `/hooks/github`, with the body as it arrived so the controller can check its signature.

use serde::{Deserialize, Serialize};

/// Arguments of `github-hook`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubHookReq {
    /// The `X-GitHub-Event` header, such as `pull_request`; empty when it was missing.
    pub event: String,
    /// The `X-GitHub-Delivery` header, for the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    /// The `X-Hub-Signature-256` header; none when it was missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// The body, byte for byte.
    pub body: String,
}

/// Value of `github-hook`: what the gate answers GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "hook", rename_all = "kebab-case")]
pub enum GitHubHookValue {
    /// The signature matched. The event is handled after the answer.
    Accepted,
    /// The signature was missing or wrong, so nothing was recorded.
    BadSignature,
    /// The controller has no App webhook secret, so it takes no deliveries.
    Off,
}
