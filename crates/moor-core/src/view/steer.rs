//! Typed `value` payload for `steer` (JSON wire shape).

/// Result of `steer`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SteerValue {
    /// Task name.
    pub task: String,
    /// What was sent: the message, or the menu choice.
    pub sent: String,
    /// Whether a menu choice was sent and the agent no longer waits at the menu.
    pub answered_menu: bool,
}
