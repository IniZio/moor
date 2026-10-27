//! Typed `value` payload for `attach` (JSON wire shape).

/// `value` of `attach`, sent after the relay's last bytes frame.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttachView {
    /// Why the relay ended.
    pub ended_by: AttachEnd,
    /// Bytes sent to the caller.
    pub bytes_out: u64,
    /// Bytes taken from the caller.
    pub bytes_in: u64,
}

/// Why an `attach` relay ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachEnd {
    /// The caller shut its side or went away.
    Client,
    /// The agent's terminal session ended.
    Agent,
    /// The link to the Box dropped, or the controller began to restart.
    Link,
}
