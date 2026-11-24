//! What a Box is tied to beyond its own row: the workflow run that made it and the node it runs on.

use serde::{Deserialize, Serialize};

/// The workflow whose run started a Box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRef {
    /// The workflow's name.
    pub name: String,
    /// The pull request the run was for, `OWNER/NAME#NUMBER`; none for a run for no pull request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<String>,
}

/// The run behind a Box, the node it runs on and the place that started it, each none when the Box
/// has no such tie.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxTies {
    /// The workflow of the Box's latest run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowRef>,
    /// The name of the node the Box runs on, which the portal calls its machine; none while it is
    /// placed on none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// The name of the claimed place whose agent started the Box; none for a Box a person or a
    /// workflow started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl BoxTies {
    /// Whether the Box is tied to none of them.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.workflow.is_none() && self.machine.is_none() && self.task.is_none()
    }
}
