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
    /// A run entered an approve step and waits for its owner.
    RunWaiting {
        /// The workflow's name.
        workflow: String,
        /// The repo the run is for.
        repo: String,
        /// The run's id.
        run: i64,
        /// The approve step.
        step: String,
        /// What the owner is asked.
        question: String,
        /// The run page, as a path on the portal.
        link: String,
    },
    /// A run ended Failed, Incomplete, `TimedOut` or Refused.
    RunFailed {
        /// The workflow's name.
        workflow: String,
        /// The repo the run is for.
        repo: String,
        /// The run's id.
        run: i64,
        /// How it ended: `failed`, `incomplete`, `timed_out` or `refused`.
        outcome: String,
        /// The step it ended at; none for a run refused before a step.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<String>,
        /// Why it ended.
        reason: String,
        /// The run page, as a path on the portal.
        link: String,
    },
}

impl WorkflowEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PreviousBoxKept { .. } => "previous_box_kept",
            Self::RunWaiting { .. } => "run_waiting",
            Self::RunFailed { .. } => "run_failed",
        }
    }
}
