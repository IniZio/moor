//! Engine seam: the agent session running in a Box.

use std::path::PathBuf;

use crate::activity::Probes;
use crate::name::{BoxHandle, BoxPath};
use crate::seam::driver::DriverError;
use crate::seam::proc::Deadline;
use crate::seam::surface::PaneRef;

/// A stored agent session and where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    /// The stored session reference.
    pub id: String,
    /// The Box the agent runs in.
    pub handle: BoxHandle,
    /// The Box workdir; the hook file lives under it.
    pub workdir: BoxPath,
    /// Host worktree; passed as the host cwd to Driver calls.
    pub worktree: PathBuf,
    /// The agent's pane, if one exists.
    pub pane: Option<PaneRef>,
}

/// An Engine failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// The hook file path could not be formed.
    #[error("box: {0}")]
    Box(DriverError),
}

/// The agent running in a Box.
pub trait Engine: Send + Sync {
    /// Reads the agent's hook file once, then its pane once, and returns them raw.
    /// It never chooses an agent state.
    ///
    /// # Errors
    /// [`EngineError::Box`] only when the hook file path cannot be formed;
    /// a failed or late read is reported in the returned [`Probes`] instead.
    fn probe(&self, s: &SessionRef, deadline: Deadline) -> Result<Probes, EngineError>;
}

#[cfg(test)]
mod tests;
