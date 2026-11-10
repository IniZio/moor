//! Typed `value` payload for `steer` (JSON wire shape).

use crate::domain::inbox::QueueCause;

/// Result of `steer`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SteerValue {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: String,
    /// What was sent: the message, or the menu choice.
    pub sent: String,
    /// Whether a menu choice was sent and the agent no longer waits at the menu.
    pub answered_menu: bool,
    /// Id of the `steered` event this steer wrote; none while it waits in the inbox.
    pub steer: Option<i64>,
    /// Whether the agent has it now, or it waits in the inbox.
    #[serde(flatten)]
    pub delivery: Delivery,
    /// The inbox row the steer is sent from.
    pub inbox: i64,
}

/// Whether a steer reached the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "delivery", content = "cause", rename_all = "snake_case")]
pub enum Delivery {
    /// The agent has it.
    Delivered,
    /// It waits in the Box's inbox for this cause, and moor sends it once the agent can take it.
    Queued(QueueCause),
}

/// Why a queued steer waits, in words for a person, from the `cause` and the `resource` waited
/// for in its reply: `waiting for room: memory`.
#[must_use]
pub fn queued_words(value: &serde_json::Value) -> String {
    let field = |k: &str| value.get(k).and_then(serde_json::Value::as_str);
    let cause = field("cause").unwrap_or("busy").replace('_', " ");
    field("resource").map_or_else(|| cause.clone(), |r| format!("{cause}: {r}"))
}

#[cfg(test)]
mod tests;
