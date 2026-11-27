//! The contract a launch gave its agent.

use serde::Serialize;

/// What moor's contract was at a fresh agent session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContractEvent {
    /// A new session read this version of the contract.
    ContractGiven {
        /// The contract's `Version N`.
        version: u32,
        /// The sha256 of the contract file, in hex.
        sha256: String,
        /// The harness that read it, such as `claude-code`.
        harness: String,
    },
}

impl ContractEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ContractGiven { .. } => "contract_given",
        }
    }
}
