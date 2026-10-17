//! Engine seam: the agent session running in a Box.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::activity::Probes;
use crate::name::{BoxHandle, BoxPath, NameError, TaskName};
use crate::seam::agent_profile::{PromptMatch, ProxyAddr};
use crate::seam::driver::DriverError;
use crate::seam::proc::Deadline;
use crate::seam::surface::{PaneRef, WorkspaceRef};
use crate::task_key::{MacLayout, TaskKey};

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
    /// The sandbox the agent runs under, such as `nono 0.80.0`; none when unsandboxed.
    pub sandbox: Option<String>,
    /// Non-fatal notices from the launch, for the caller to report; empty otherwise.
    pub warnings: Vec<LaunchNotice>,
}

/// A non-fatal notice from a launch; the caller reports it as a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchNotice {
    /// Stable machine code.
    pub code: String,
    /// Human message.
    pub message: String,
}

/// What a launch needs to start the agent in a Box.
#[derive(Debug, Clone)]
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
    /// The Proxy the agent reaches GitHub and its model through; none means no Proxy env and no bypass.
    pub proxy: Option<Arc<ProxyAddr>>,
    /// The task repo's git origin URL; GitHub push and `gh` routing apply only when it names a GitHub repo.
    pub origin: Option<String>,
    /// Env var names removed from the agent's environment before hooks and the agent run.
    pub strip_env: Vec<String>,
}

impl LaunchReq {
    /// The credential and tmp directories this launch creates: under `launch/` and `tmp/` of a
    /// macOS Box layout when the workdir is inside one, else under the Box home `home`.
    ///
    /// # Errors
    /// [`NameError`] when `home` and the Task name do not form a valid Box path.
    pub fn dirs_under(&self, home: &BoxPath) -> Result<(BoxPath, BoxPath), NameError> {
        if let Some(layout) = MacLayout::find(&self.workdir, &TaskKey::new(&self.task)) {
            return Ok((layout.cred_dir(&self.nonce)?, layout.cred_tmp(&self.nonce)?));
        }
        let (cred, tmp) = self.dir_names();
        Ok((
            BoxPath::under(home.as_str(), &cred)?,
            BoxPath::under(home.as_str(), &tmp)?,
        ))
    }

    /// The names of the credential and tmp directories this launch creates under the Box home.
    #[must_use]
    pub fn dir_names(&self) -> (String, String) {
        let (task, nonce) = (self.task.as_str(), &self.nonce);
        (
            format!(".moor-cred-{task}-{nonce}"),
            format!(".moor-tmp-{task}-{nonce}"),
        )
    }
}

impl PartialEq for LaunchReq {
    /// Proxies compare by port and nonce digest, since the nonce itself is not comparable.
    fn eq(&self, other: &Self) -> bool {
        let proxy = |p: &Option<Arc<ProxyAddr>>| p.as_ref().map(|a| (a.box_port, a.nonce.digest()));
        self.handle == other.handle
            && self.workdir == other.workdir
            && self.worktree == other.worktree
            && self.workspace == other.workspace
            && self.task == other.task
            && self.branch == other.branch
            && self.goal == other.goal
            && self.resume == other.resume
            && self.issue == other.issue
            && self.setup_exit == other.setup_exit
            && self.model == other.model
            && self.nonce == other.nonce
            && proxy(&self.proxy) == proxy(&other.proxy)
            && self.origin == other.origin
            && self.strip_env == other.strip_env
    }
}

impl Eq for LaunchReq {}

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
    /// The agent left no transcript to archive.
    #[error("transcript: nothing to archive")]
    NothingToArchive,
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

    /// The credential and tmp directories `launch` of `req` creates on the Box; `Ok(None)`
    /// when this engine creates none.
    ///
    /// # Errors
    /// [`EngineError`] when they cannot be named now (for example the Box home cannot be
    /// read); the caller must not launch then.
    fn launch_dirs(&self, req: &LaunchReq) -> Result<Option<(BoxPath, BoxPath)>, EngineError>;

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
    /// [`EngineError::Transcript`] names the step that failed; [`EngineError::NothingToArchive`]
    /// when the agent left no transcript.
    fn transcript(&self, s: &SessionRef, dest: &Path) -> Result<(), EngineError>;
}

#[cfg(test)]
mod tests;
