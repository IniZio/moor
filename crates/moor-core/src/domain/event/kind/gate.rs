//! Events about a Box's gates.

use serde::Serialize;

use crate::gate::WaitKind;

/// A gate changed what the owner has to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GateEvent {
    /// A gate began to wait for a person: the owner's yes or no, or a steer for a stuck gate.
    GateWaiting {
        /// The gate.
        gate: String,
        /// What the person is asked.
        wait: WaitKind,
        /// The number of gate lines written when the wait began; the wait is over once more are.
        seq: u32,
        /// What the owner is told, in a sentence.
        question: String,
    },
}

impl GateEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::GateWaiting { .. } => "gate_waiting",
        }
    }
}
