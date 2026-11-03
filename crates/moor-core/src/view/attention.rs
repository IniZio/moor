//! A problem with the agent's model account that needs the user.

/// A problem with the agent's model account that needs the user.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AttentionView {
    /// Problem kind: `agent_rate_limited` or `proxy_credential_rejected`.
    pub kind: String,
    /// Credential rule the problem belongs to.
    pub rule: String,
    /// Upstream HTTP status.
    pub status: u16,
    /// When the limit resets, if known.
    pub resets_at: Option<String>,
    /// Human-readable summary.
    pub message: String,
}
