//! Engine seam: the agent session running in a Box.

use std::path::Path;
use std::sync::Arc;

use crate::activity::Probes;
use crate::box_key::BoxKey;
use crate::name::{BoxHandle, BoxId, BoxName, BoxPath, NameError};
use crate::seam::agent_profile::{PromptMatch, ProxyAddr};
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
    /// The agent's pane, if one exists.
    pub pane: Option<PaneRef>,
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

/// The first prompt the agent starts with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LaunchPrompt {
    /// The prompt derived from the brief.
    #[default]
    Brief,
    /// This text, verbatim.
    Text(String),
    /// No first prompt; the agent waits for input.
    None,
}

/// What a launch needs to start the agent in a Box.
#[derive(Debug, Clone)]
pub struct LaunchReq {
    /// The Box's key; its internal dirs derive from it.
    pub id: BoxId,
    /// The Box to run in.
    pub handle: BoxHandle,
    /// The Box workdir the agent runs in.
    pub workdir: BoxPath,
    /// The Workspace that gets the agent's tab.
    pub workspace: WorkspaceRef,
    /// The Box the agent works on.
    pub box_name: BoxName,
    /// The Box branch the agent pushes to.
    pub branch: String,
    /// What the agent is asked to do.
    pub goal: Option<String>,
    /// The stored agent session to continue instead of reading the brief.
    pub resume: Option<String>,
    /// The Issue text, when the Box came from one.
    pub issue: Option<String>,
    /// The exit code of a failed setup, if setup failed.
    pub setup_exit: Option<i32>,
    /// The model to ask for; the agent's default when `None`.
    pub model: Option<String>,
    /// Random hex that keeps the credential directory name unique.
    pub nonce: String,
    /// The Proxy the agent reaches GitHub and its model through; none means no Proxy env and no bypass.
    pub proxy: Option<Arc<ProxyAddr>>,
    /// Env var names removed from the agent's environment before hooks and the agent run.
    pub strip_env: Vec<String>,
    /// The first prompt the agent starts with.
    pub prompt: LaunchPrompt,
}

impl LaunchReq {
    /// The credential and tmp directories this launch creates under the Box home `home`.
    ///
    /// # Errors
    /// [`NameError`] when `home` and the Box name do not form a valid Box path.
    pub fn dirs_under(&self, home: &BoxPath) -> Result<(BoxPath, BoxPath), NameError> {
        let (cred, tmp) = self.dir_names();
        Ok((
            BoxPath::under(home.as_str(), &cred)?,
            BoxPath::under(home.as_str(), &tmp)?,
        ))
    }

    /// The names of the credential and tmp directories this launch creates under the Box home.
    #[must_use]
    pub fn dir_names(&self) -> (String, String) {
        let key = BoxKey::new(self.id);
        (key.cred_dir(&self.nonce), key.tmp_dir(&self.nonce))
    }
}

impl PartialEq for LaunchReq {
    /// Proxies compare by port.
    fn eq(&self, other: &Self) -> bool {
        let proxy = |p: &Option<Arc<ProxyAddr>>| p.as_ref().map(|a| a.box_port);
        self.id == other.id
            && self.handle == other.handle
            && self.workdir == other.workdir
            && self.workspace == other.workspace
            && self.box_name == other.box_name
            && self.branch == other.branch
            && self.goal == other.goal
            && self.resume == other.resume
            && self.issue == other.issue
            && self.setup_exit == other.setup_exit
            && self.model == other.model
            && self.nonce == other.nonce
            && proxy(&self.proxy) == proxy(&other.proxy)
            && self.strip_env == other.strip_env
            && self.prompt == other.prompt
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
    /// The Box has no agent session.
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

    /// Whether the agent runs no hook while a first-run screen holds its pane.
    fn hooks_quiet_during_first_run(&self) -> bool;

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
