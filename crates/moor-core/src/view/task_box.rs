//! A work Box a place's agent started, as the task that asked for it reads it.

use serde::{Deserialize, Serialize};

use super::ArtifactView;
use crate::domain::box_state::BoxLifecycle;

/// Where a work Box stands, in the words a task shows. The Box's own lifecycle maps onto these
/// five, so a task never has to name a rejected or a setup-failed Box differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskBoxState {
    /// Waiting for a slot, or coming up.
    Starting,
    /// Up, and open for code to run in.
    Running,
    /// Asleep; the next exec wakes it.
    Asleep,
    /// Closed; its files are kept as artifacts.
    Closed,
    /// Rejected, or setup failed.
    Failed,
}

impl From<BoxLifecycle> for TaskBoxState {
    fn from(lifecycle: BoxLifecycle) -> Self {
        match lifecycle {
            BoxLifecycle::Checking | BoxLifecycle::Pending | BoxLifecycle::SettingUp => {
                Self::Starting
            }
            BoxLifecycle::Ready => Self::Running,
            BoxLifecycle::Asleep => Self::Asleep,
            BoxLifecycle::Closed => Self::Closed,
            BoxLifecycle::Rejected | BoxLifecycle::SetupFailed => Self::Failed,
        }
    }
}

/// One work Box of a task: the line the conversation shows for it and the files it left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBoxView {
    /// The Box's name; its artifacts and terminal are addressed by it.
    pub name: String,
    /// The repo the Box cloned, `OWNER/NAME`.
    pub repo: String,
    /// RFC 3339 UTC time the Box was started.
    pub started_at: String,
    /// Milliseconds from the start to the Box's first `working` event, which is when its
    /// provisioning ended; none before then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_ms: Option<u64>,
    /// Where the Box stands.
    pub state: TaskBoxState,
    /// RFC 3339 UTC time of the Box's `closed` event; none while it is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    /// The latest version of each artifact the Box saved.
    #[serde(default)]
    pub artifacts: Vec<ArtifactView>,
}
