//! Events a decision may carry, and their conversion to and from stored events.

use crate::domain::event::{CheckedCred, TurnOutcome};
use crate::name::RunId;

/// An event a caller may pass; every other kind is derived from state steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraEvent {
    /// The driver refused to sleep an idle box, so it keeps running.
    StaysUp {
        /// The driver's reason.
        reason: String,
    },
    /// A command run finished.
    Ran {
        /// The run id; none when the run was refused before a run row existed.
        run: Option<RunId>,
        /// The exit code, when known.
        exit: Option<i32>,
        /// True when the run was interrupted.
        interrupted: bool,
        /// Why the run failed before the command reported an exit.
        error: Option<String>,
    },
    /// Files were copied.
    Copied {
        /// The copy source.
        from: String,
        /// The copy destination.
        to: String,
    },
    /// An agent session started.
    SessionStarted {
        /// The session id.
        session: String,
    },
    /// The agent launch failed; the Box stays in its state, so no step derives it.
    SessionFailed {
        /// The launch step that failed.
        step: String,
        /// What went wrong.
        message: String,
    },
    /// The agent did not start within `box.launch_timeout`.
    LaunchTimedOut {
        /// The configured limit, in seconds.
        after_secs: u64,
    },
    /// moor refused another automatic agent relaunch.
    RelaunchCapped {
        /// Relaunches within the window.
        count: usize,
        /// The window, in seconds.
        window_secs: u64,
    },
    /// The agent was started again after its pane ended.
    Relaunched {
        /// The pane of the session that ended.
        old_pane: Option<String>,
        /// The pane of the new session.
        new_pane: Option<String>,
        /// The goal the new session works on.
        goal: Option<String>,
        /// Why the agent was relaunched; `None` for a plain `moor start`.
        cause: Option<String>,
    },
    /// Setup failed on a Box that stays in its state, so no step derives it.
    SetupFailed {
        /// The failing output line.
        line: String,
    },
    /// The repo setup script ended.
    SetupFinished {
        /// Time the script took, in milliseconds.
        ms: u64,
        /// Whether every line passed.
        ok: bool,
    },
    /// The repo was cloned in the Box.
    Cloned {
        /// Time the clone, with the git identity set inside it, took in milliseconds.
        ms: u64,
    },
    /// Admission checked credentials.
    Checked {
        /// The credentials that were checked.
        credentials: Vec<CheckedCred>,
    },
    /// A command run started.
    RunStarted {
        /// The run id.
        run: RunId,
    },
    /// A start attached to an existing open box.
    Attached,
    /// Close ended the live agent sessions.
    SessionsStopped {
        /// The ended session ids.
        sessions: Vec<String>,
    },
    /// The agent waits at a first-run prompt.
    AgentPromptBlocked {
        /// The agent's pane.
        pane: String,
        /// The prompt, by name.
        prompt: String,
        /// The pane line that showed it.
        line: String,
    },
    /// The agent's last message before it went quiet.
    AgentSaid {
        /// The message, cut to its end when long.
        text: String,
    },
    /// What the agent is doing now.
    AgentDoing {
        /// The line, at most 80 characters.
        text: String,
    },
    /// The agent stopped to ask the user something.
    NeedsInput {
        /// The question, when the agent showed one.
        question: Option<String>,
        /// The offered answers.
        options: Vec<String>,
    },
    /// The Box could not be reached to tell whether the agent outlived its pane.
    AgentUnreachable {
        /// Tries made before giving up.
        attempts: u32,
        /// The last error.
        error: String,
    },
    /// Reading the agent's pane for first-run prompts failed.
    AgentPromptCheckFailed {
        /// The agent's pane.
        pane: String,
        /// What went wrong.
        error: String,
    },
    /// The agent's transcript was saved on the host.
    TranscriptSaved {
        /// What ended the session: `relaunch` or `close`.
        trigger: String,
        /// The host path of the archive.
        path: String,
        /// The digest of the archive.
        digest: String,
    },
    /// The agent's transcript could not be saved.
    TranscriptFailed {
        /// What ended the session: `relaunch` or `close`.
        trigger: String,
        /// What went wrong.
        error: String,
    },
    /// The agent's transcript was not saved because there was nothing to save.
    TranscriptSkipped {
        /// What ended the session: `relaunch` or `close`.
        trigger: String,
        /// Why nothing was saved.
        reason: String,
    },
    /// A message or menu choice was sent to the agent.
    Steered {
        /// The agent session it went to.
        session: String,
        /// What was sent.
        message: String,
        /// Whether the submit key was pressed a second time.
        submit_retried: bool,
        /// When the newest hook report read before the send was written, in Box-clock ms.
        hook_before: Option<i64>,
        /// Whether the steer typed a prompt that starts a turn of its own.
        new_turn: bool,
    },
    /// The agent finished the turn a steer started.
    TurnDone {
        /// The steer's id.
        steer: i64,
        /// How the turn ended.
        outcome: TurnOutcome,
    },
    /// The Proxy for the box is ready to accept connections.
    ProxyStarted {
        /// Port the Box side connects to.
        box_port: u16,
        /// Process id of the Proxy.
        pid: u32,
        /// Opaque handle that identifies the Proxy process to its runner.
        handle: String,
    },
    /// The tunnel between the Box and the Proxy was restarted.
    TunnelRestarted {
        /// Port the Box side connects to.
        box_port: u16,
        /// Why the tunnel was restarted.
        reason: String,
    },
}

mod convert;
