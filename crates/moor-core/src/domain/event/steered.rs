//! The record of a message or menu choice sent to a Box's agent.

/// The sender an inbox row carries when moor itself wrote the text, such as a gate's steer.
pub const MOOR_SENDER: &str = "moor";

/// Who wrote a message sent to the agent.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A person: whoever steered, or the one who started the Box.
    #[default]
    Person,
    /// moor, such as its steer about a gate.
    Moor,
}

impl Origin {
    /// Who wrote the text of an inbox row from `sender`.
    #[must_use]
    pub fn of_sender(sender: &str) -> Self {
        if sender == MOOR_SENDER {
            Self::Moor
        } else {
            Self::Person
        }
    }

    /// Whether a person wrote it, which a `steered` event leaves unsaid.
    #[must_use]
    pub const fn is_person(&self) -> bool {
        matches!(self, Self::Person)
    }
}

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
    /// Who wrote the message; a person when the event does not say.
    #[serde(default, skip_serializing_if = "Origin::is_person")]
    pub origin: Origin,
}
