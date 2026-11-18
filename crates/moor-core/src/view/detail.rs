//! What `show` adds to the detail of one Box: its agent's newest message and the approve step
//! that waits in it.

use serde::{Deserialize, Serialize};

/// The newest `agent_said` event of a Box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaidView {
    /// What the agent said.
    pub text: String,
    /// RFC3339 UTC time it was recorded.
    pub at: String,
}

/// The approve step of a workflow run that waits in a Box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalView {
    /// The workflow's name.
    pub workflow: String,
    /// The run's id.
    pub run: i64,
    /// What the step asks its owner.
    pub question: String,
}
