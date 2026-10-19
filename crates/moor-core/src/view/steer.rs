//! Typed `value` payload for `steer` (JSON wire shape).

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
    /// Id of the `steered` event this steer wrote.
    pub steer: i64,
}
