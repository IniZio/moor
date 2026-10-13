//! Engine seam: the agent session running in a Box.

use std::path::{Path, PathBuf};

use crate::activity::Probes;
use crate::name::{BoxHandle, BoxPath, TaskName};
use crate::seam::agent_profile::PromptMatch;
use crate::seam::driver::DriverError;
use crate::seam::proc::Deadline;
use crate::seam::surface::{PaneRef, WorkspaceRef};

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

/// What a launch needs to start the agent in a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchReq {
    /// The Box to run in.
    pub handle: BoxHandle,
    /// The Box workdir the agent runs in.
    pub workdir: BoxPath,
    /// Host worktree; passed as the host cwd to Driver calls and the pane.
    pub worktree: PathBuf,
    /// The Workspace that gets the agent's tab.
    pub workspace: WorkspaceRef,
    /// The Task the agent works on.
    pub task: TaskName,
    /// The Task branch the agent pushes to.
    pub branch: String,
    /// What the agent is asked to do.
    pub goal: Option<String>,
    /// The stored agent session to continue instead of reading the brief.
    pub resume: Option<String>,
    /// The Issue text, when the Task came from one.
    pub issue: Option<String>,
    /// The exit code of a failed setup, if setup failed.
    pub setup_exit: Option<i32>,
    /// The model to ask for; the agent's default when `None`.
    pub model: Option<String>,
    /// Random hex that keeps the credential directory name unique.
    pub nonce: String,
}

/// What to send to the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Steer {
    /// A message typed into the agent's input and submitted.
    Text(String),
    /// A choice in a menu the agent shows.
    Menu(u8),
}

/// What a steer did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteerOutcome {
    /// Whether the submit key was pressed a second time because the text stayed in the input.
    pub submit_retried: bool,
    /// Whether the pane was read again after submitting text; `false` means the send is unconfirmed,
    /// as for a menu choice or a failed read.
    pub submit_checked: bool,
}

/// An Engine failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// The hook file path could not be formed.
    #[error("box: {0}")]
    Box(DriverError),
    /// A launch step failed; a credential directory that could not be removed is named in `message`.
    #[error("launch failed at {step}: {message}")]
    Failed {
        /// The step that failed.
        step: &'static str,
        /// What went wrong.
        message: String,
    },
    /// The agent's pane no longer exists.
    #[error("the agent pane is gone")]
    PaneGone,
    /// The Task has no agent session.
    #[error("no agent session")]
    NoSession,
    /// The transcript could not be archived or copied out.
    #[error("transcript: {0}")]
    Transcript(String),
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

    /// The first-run prompt the pane text `tail` shows, when one holds the agent.
    fn first_run_prompt(&self, tail: &str) -> Option<PromptMatch>;

    /// Delivers the brief and hooks to the Box and opens the agent's pane. The returned
    /// session has an empty `id` until the agent reports its own.
    ///
    /// # Errors
    /// [`EngineError::Failed`] names the step that failed; no pane is open then, and the
    /// credential directory is removed.
    fn launch(&self, req: &LaunchReq) -> Result<SessionRef, EngineError>;

    /// Sends `steer` to the agent's pane. Text is never typed twice.
    ///
    /// # Errors
    /// [`EngineError::NoSession`] when the session has no pane; [`EngineError::PaneGone`]
    /// when the pane no longer exists or shows that the agent exited;
    /// [`EngineError::Failed`] when a send fails.
    fn steer(
        &self,
        s: &SessionRef,
        steer: &Steer,
        deadline: Deadline,
    ) -> Result<SteerOutcome, EngineError>;

    /// Archives the agent's transcript in the Box and copies the archive to `dest` on the host.
    ///
    /// # Errors
    /// [`EngineError::Transcript`] names the step that failed.
    fn transcript(&self, s: &SessionRef, dest: &Path) -> Result<(), EngineError>;
}

#[cfg(test)]
mod tests;
