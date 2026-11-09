//! The record of a message or menu choice sent to a Box's agent.

/// A message or menu choice sent to the agent, and where its terminal was at the send.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SteerSent {
    /// The agent session it went to.
    pub session: String,
    /// What was sent.
    pub message: String,
    /// Whether the submit key was pressed a second time.
    pub submit_retried: bool,
    /// The ended-turn count of the agent's terminal the steer's turn must pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_before: Option<u64>,
    /// When the status the agent showed at the send was set, on the Box's clock, in
    /// milliseconds; none when a turn was running then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_before: Option<i64>,
    /// The inbox row it was sent from, which waits until its turn ends; none on a steer sent
    /// before steers had rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox: Option<i64>,
}
