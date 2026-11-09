//! Events about a workflow's run.

use serde::Serialize;

/// A workflow's earlier Box stayed open when its next run started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowEvent {
    /// The Box of an earlier run was not closed, so it is still there to look at.
    PreviousBoxKept {
        /// The kept Box.
        name: String,
        /// Why it was not closed.
        reason: String,
    },
}

impl WorkflowEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PreviousBoxKept { .. } => "previous_box_kept",
        }
    }
}
