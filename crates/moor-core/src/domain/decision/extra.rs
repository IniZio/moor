//! Events a decision may carry, and their conversion to and from stored events.

use crate::domain::event::{CheckedCred, PostCloneStep, ProfileStep, TurnOutcome};
use crate::name::RunId;

/// An event a caller may pass; every other kind is derived from state steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraEvent {
    /// The driver refused or failed to sleep a box, so it keeps running.
    StaysUp {
        /// The driver's reason.
        reason: String,
    },
    /// The Box's machine was gone and a new one replaced it, without the old disk.
    DiskLost,
    /// A close deleted the Box without waking it, so its caches were not saved.
    CachesNotSaved,
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
        /// The prompt the new session works on.
        prompt: Option<String>,
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
    /// A step after the clone began.
    StepStarted {
        /// Which step.
        step: PostCloneStep,
    },
    /// A step after the clone ended.
    StepFinished {
        /// Which step.
        step: PostCloneStep,
        /// Time the step took, in milliseconds.
        ms: u64,
        /// Whether the step succeeded.
        ok: bool,
    },
    /// One of the repo's `setup:` lines began.
    SetupLineStarted {
        /// Position of the line, counting from 1.
        index: u32,
        /// How many lines the repo declares.
        total: u32,
        /// The line, cut to a length a status line can hold.
        line: String,
    },
    /// One of the repo's `setup:` lines ended.
    SetupLineFinished {
        /// Position of the line, counting from 1.
        index: u32,
        /// How many lines the repo declares.
        total: u32,
        /// Time the line took, in milliseconds.
        ms: u64,
        /// Whether the line exited 0.
        ok: bool,
    },
    /// The Box started on a fork of the deepest setup checkpoint an earlier Box saved.
    WarmStarted {
        /// Setup lines the checkpoint already holds; 0 is the clone alone.
        layer: u32,
        /// How many lines the repo declares.
        layers: u32,
        /// What the clone and those lines took the Box that saved them, less the fork.
        saved_ms: u64,
        /// Time the fork took, in milliseconds.
        fork_ms: u64,
        /// What the provider reports the fork spent that time on.
        steps: Vec<crate::domain::event::ProviderStep>,
        /// The commit the checkpoint's disk holds, when it is not the Box's base, so every line
        /// runs again; none when the Box stands on those lines.
        rerun_from: Option<String>,
    },
    /// The Box reran its setup lines over a checkpoint of another commit.
    WarmRerun {
        /// The commit the checkpoint's disk holds.
        commit: String,
        /// How many tracked paths differ from that commit; none when git could not tell.
        changed: Option<u64>,
        /// The ignored paths that came with the disk.
        ignored: Vec<String>,
    },
    /// A line failed on a rerun over a checkpoint, so the Box is set up from scratch once.
    WarmRerunFailed {
        /// The line that failed.
        line: String,
        /// The last lines of its log.
        tail: String,
    },
    /// The fork of a setup checkpoint failed, so the Box is set up from scratch.
    WarmStartFailed {
        /// Setup lines the checkpoint held.
        layer: u32,
        /// What the provider said.
        error: String,
    },
    /// The Box started without cache volumes although its plan would have kept some.
    CacheCold {
        /// Why, as a snake-case name.
        reason: String,
        /// The reason in a sentence.
        detail: String,
    },
    /// A restored cache volume failed a restore step, so the Box emptied it and went on.
    CacheEmptied {
        /// The volume.
        volume: String,
        /// Why, as the step printed it.
        reason: String,
    },
    /// A setup layer's checkpoint was taken while the Box was paused, or the attempt failed.
    CheckpointTaken {
        /// Setup lines the checkpoint holds; 0 is the clone alone.
        layer: u32,
        /// The layer's key.
        key: String,
        /// The provider's checkpoint id; none when the attempt failed.
        checkpoint: Option<String>,
        /// Time the Box was paused for it, in milliseconds.
        duration_ms: u64,
        /// What the provider said when the attempt failed.
        error: Option<String>,
    },
    /// A setup checkpoint was deleted to keep the provider's disk above its floor.
    CheckpointEvicted {
        /// The provider's checkpoint id.
        checkpoint: String,
        /// The repo whose setup it held, `OWNER/NAME`.
        repo: String,
        /// Its size on the host, when known.
        bytes: Option<u64>,
        /// Why it went before others: `superseded` or `least_recently_used`.
        reason: String,
        /// Free disk when it was picked, in MiB.
        free_mib: u64,
        /// The free disk the floor asks for, in MiB.
        needs_mib: u64,
    },
    /// The repo was cloned in the Box.
    Cloned {
        /// Time the clone, with the git identity set inside it, took in milliseconds.
        ms: u64,
    },
    /// The requester's agent profile resolved to a commit.
    ProfileResolved {
        /// The profile repo, `owner/name`.
        repo: crate::name::ForgeRepo,
        /// The full commit sha.
        sha: String,
    },
    /// A profile step failed; the Box starts without the profile.
    ProfileFailed {
        /// Which step failed.
        step: ProfileStep,
        /// What went wrong.
        detail: String,
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
    /// What the provider reports while it brings the Box up.
    BoxProgress {
        /// The step or the latest problem, one short line.
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
    /// The agent's hook reported busy and then went quiet for too long.
    AgentStalled {
        /// What the agent was doing at its last report.
        doing: Option<String>,
        /// Whole seconds since that report.
        quiet_secs: u64,
    },
    /// The agent reported again after a stall.
    AgentResumed {
        /// Whole seconds the agent had been quiet.
        quiet_secs: u64,
    },
    /// The disk of the node the Box runs on fell below what it keeps for another Box.
    NodeDiskLow {
        /// The node.
        node: String,
        /// Free MiB.
        free_mib: u64,
        /// The filesystem's size in MiB.
        total_mib: u64,
    },
    /// The disk of the node the Box runs on climbed back past its floor and margin.
    NodeDiskRecovered {
        /// The node.
        node: String,
        /// Free MiB.
        free_mib: u64,
        /// The filesystem's size in MiB.
        total_mib: u64,
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
        /// The Box's machine was not linked; the save is tried again once the link is back.
        unlinked: bool,
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
mod try_from;
