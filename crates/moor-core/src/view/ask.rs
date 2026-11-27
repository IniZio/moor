//! Typed `value` payload for `ask` (JSON wire shape).

/// Result of `ask`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AskValue {
    /// The place that took the question.
    #[serde(rename = "box")]
    pub box_name: String,
    /// When the question took the place, in milliseconds since the epoch.
    pub claimed_at: i64,
    /// When the place was given the repos' rules; none while that waits.
    pub served_at: Option<i64>,
    /// When the question reached the agent; none while it waits in the inbox.
    pub steered_at: Option<i64>,
}
