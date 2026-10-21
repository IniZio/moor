//! Surface seam: worktrees, workspaces and panes behind a terminal multiplexer.

use std::path::{Path, PathBuf};

use crate::activity::{PaneObs, PaneStatus};
use crate::name::BoxName;
use crate::seam::proc::Deadline;

/// A checked-out worktree and its branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Absolute path of the worktree.
    pub path: PathBuf,
    /// Branch checked out in the worktree.
    pub branch: String,
}

/// An opaque handle to a Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceRef(String);

impl WorkspaceRef {
    /// Wraps a raw handle.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the handle text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An opaque handle to a pane.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PaneRef(String);

impl PaneRef {
    /// Wraps a raw handle.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the handle text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The command a pane runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaneCmd {
    /// Label of the tab that holds the pane.
    pub label: String,
    /// Program and arguments.
    pub argv: Vec<String>,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
}

/// What the multiplexer needs to restart an agent session in a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeReg {
    /// Agent label.
    pub agent: String,
    /// Status reported with the registration.
    pub state: PaneStatus,
    /// The agent's session id.
    pub session_id: String,
    /// The box `moor start --resume` restarts.
    pub box_name: BoxName,
}

/// How the multiplexer answered a resume registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// The resume command is registered.
    Registered,
    /// The multiplexer declined the registration, with its reason code.
    Rejected(String),
}

/// A key press sent to a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Control-U: clears the input line.
    CtrlU,
    /// Control-M: submits the input line.
    CtrlM,
    /// A digit key, 0 to 9.
    Digit(u8),
}

impl Key {
    /// The key's name as the multiplexer spells it, `None` for a digit above 9.
    #[must_use]
    pub fn name(self) -> Option<String> {
        match self {
            Self::CtrlU => Some("ctrl+u".to_owned()),
            Self::CtrlM => Some("ctrl+m".to_owned()),
            Self::Digit(d) => char::from_digit(u32::from(d), 10).map(String::from),
        }
    }
}

/// A Surface failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SurfaceError {
    /// The worktree, workspace or pane does not exist.
    #[error("not found")]
    NotFound,
    /// The call missed its deadline.
    #[error("timed out")]
    Timeout,
    /// The surface backend cannot be reached.
    #[error("surface unavailable: {0}")]
    Unavailable(String),
    /// Creating the worktree failed because one is left on the Box's branch.
    #[error("a worktree is left at {}", path.display())]
    Leftover {
        /// Where the leftover worktree is.
        path: PathBuf,
    },
    /// The backend refused or failed the call.
    #[error("surface call failed: {summary}")]
    Failed {
        /// One-line failure summary.
        summary: String,
    },
}

/// Returns the Workspace label for a box, `moor:<name>`.
#[must_use]
pub fn workspace_label(name: &BoxName) -> String {
    format!("moor:{}", name.as_str())
}

/// Worktrees, Workspaces and panes.
pub trait Surface: Send + Sync {
    /// Creates a worktree for `name` in `repo`, branching from `base` when given.
    ///
    /// # Errors
    /// Returns a [`SurfaceError`] when the worktree cannot be created.
    fn create_worktree(
        &self,
        repo: &Path,
        name: &BoxName,
        base: Option<&str>,
    ) -> Result<Worktree, SurfaceError>;

    /// Removes the worktree at `wt`.
    ///
    /// # Errors
    /// Returns a [`SurfaceError`] when the worktree cannot be removed.
    fn remove_worktree(&self, wt: &Path) -> Result<(), SurfaceError>;

    /// Returns the Workspace labelled `label` for `wt`, creating it when absent.
    ///
    /// # Errors
    /// Returns a [`SurfaceError`] when the Workspace cannot be ensured.
    fn ensure_workspace(&self, wt: &Worktree, label: &str) -> Result<WorkspaceRef, SurfaceError>;

    /// Brings the Workspace to the front.
    ///
    /// # Errors
    /// Returns a [`SurfaceError`] when the Workspace cannot be focused.
    fn focus_workspace(&self, ws: &WorkspaceRef) -> Result<(), SurfaceError>;

    /// Opens a pane running `cmd` in `ws`.
    ///
    /// # Errors
    /// Returns a [`SurfaceError`] when the pane cannot be opened.
    fn open_pane(&self, ws: &WorkspaceRef, cmd: &PaneCmd) -> Result<PaneRef, SurfaceError>;

    /// Observes `pane`: the multiplexer's status and the end of its text.
    ///
    /// # Errors
    /// Returns [`SurfaceError::NotFound`] when the pane is gone and
    /// [`SurfaceError::Timeout`] when a call overruns `deadline`. Each of the
    /// backend's two calls gets the full `deadline`, so the worst case is about twice it.
    fn pane_obs(&self, pane: &PaneRef, deadline: Deadline) -> Result<PaneObs, SurfaceError>;

    /// Types `text` into `pane` without submitting it.
    ///
    /// # Errors
    /// Returns [`SurfaceError::NotFound`] when the pane is gone.
    fn send_text(&self, pane: &PaneRef, text: &str) -> Result<(), SurfaceError>;

    /// Presses `keys` in `pane`, in order.
    ///
    /// # Errors
    /// Returns [`SurfaceError::NotFound`] when the pane is gone.
    fn send_keys(&self, pane: &PaneRef, keys: &[Key]) -> Result<(), SurfaceError>;

    /// Registers how to restart the agent session in `pane`.
    ///
    /// A refusal by the multiplexer is [`ResumeOutcome::Rejected`], not an error.
    ///
    /// # Errors
    /// Returns [`SurfaceError::NotFound`] when the pane is gone.
    fn register_resume(
        &self,
        pane: &PaneRef,
        reg: &ResumeReg,
    ) -> Result<ResumeOutcome, SurfaceError>;

    /// Closes `pane`.
    ///
    /// # Errors
    /// Returns [`SurfaceError::NotFound`] when the pane is already gone.
    fn close_pane(&self, pane: &PaneRef) -> Result<(), SurfaceError>;

    /// Closes `ws` and its panes.
    ///
    /// # Errors
    /// Returns [`SurfaceError::NotFound`] when the Workspace is already gone.
    fn close_workspace(&self, ws: &WorkspaceRef) -> Result<(), SurfaceError>;
}

#[cfg(test)]
mod tests;
