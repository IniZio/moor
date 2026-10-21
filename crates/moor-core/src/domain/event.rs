//! Typed box events. State events are derived from steps, so a row and its event cannot disagree.

use serde::Serialize;

use crate::domain::box_state::BoxLifecycle;
pub use crate::domain::turn::TurnOutcome;
use crate::name::RunId;

/// The event kinds the event tests pin. Frozen.
pub const ASSERTED_KINDS: [&str; 20] = [
    "started",
    "rejected",
    "waiting_for_box",
    "setting_up",
    "working",
    "setup_failed",
    "idled",
    "busy",
    "asleep",
    "woke",
    "closed",
    "box_ready",
    "box_failed",
    "box_gone",
    "box_deleted",
    "box_asleep",
    "session_started",
    "ran",
    "copied",
    "credential_failed",
];

/// One recorded fact about a box, serialized with its `kind` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// The box row was created.
    Started,
    /// Admission refused the box.
    Rejected {
        /// Why the box was refused.
        reason: String,
    },
    /// The Box waits for a free slot.
    WaitingForBox {
        /// The capacity limit that was hit.
        max_boxes: u32,
    },
    /// Box setup began.
    SettingUp {
        /// The state the box left.
        from: BoxLifecycle,
    },
    /// The agent is working.
    Working,
    /// Setup failed.
    SetupFailed {
        /// The failing output line or cause.
        line: String,
    },
    /// The repo setup script ended.
    SetupFinished {
        /// Time the script took, in milliseconds.
        ms: u64,
        /// Whether every line passed.
        ok: bool,
    },
    /// A credential could not be delivered.
    CredentialFailed {
        /// Which credential failed.
        which: String,
        /// The error text.
        error: String,
    },
    /// The agent went quiet.
    Idled,
    /// The agent became busy again.
    Busy,
    /// The box went to sleep.
    Asleep {
        /// True when the sleep timer fired, false for an explicit command.
        auto: bool,
    },
    /// The box woke.
    Woke,
    /// The box closed.
    Closed {
        /// The state the box left.
        from: BoxLifecycle,
    },
    /// A box was requested.
    BoxRequested,
    /// The box is ready.
    BoxReady,
    /// Acquiring the box failed.
    BoxFailed {
        /// The failure, in moor's words.
        error: String,
        /// The attempted Box handle, when the Driver reported one.
        handle: Option<String>,
    },
    /// The box went to sleep.
    BoxAsleep,
    /// The driver no longer has the box.
    BoxGone,
    /// The box was deleted.
    BoxDeleted,
    /// Releasing the box failed.
    BoxReleaseFailed {
        /// The failure, in moor's words.
        error: String,
        /// The Box handle kept for manual release.
        handle: Option<String>,
    },
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
        #[serde(skip_serializing_if = "Option::is_none")]
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
    /// A session ended while the Box stayed open.
    SessionEnded {
        /// The sessions row id.
        session: i64,
        /// Why it ended, such as `exited`, `relaunch` or `requested`.
        cause: String,
        /// The exit code, if known.
        exit: Option<i32>,
    },
    /// The agent launch failed, so no session started.
    SessionFailed {
        /// The launch step that failed.
        step: String,
        /// What went wrong.
        message: String,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        cause: Option<String>,
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
    /// The agent stopped to ask the user something; one per waiting episode.
    NeedsInput {
        /// The question, when the agent showed one.
        question: Option<String>,
        /// The offered answers, empty for a free-text question.
        options: Vec<String>,
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
    /// The Box's Proxy refused a connection to a host not allowed for the Box.
    EgressRefused {
        /// The host the connection was for.
        host: String,
        /// The tool rule the host belongs to, when it belongs to one.
        rule: Option<String>,
        /// The HTTP method of the refused request, when one was seen.
        method: Option<String>,
        /// Why the Proxy refused the connection, as the Proxy names it.
        category: Option<String>,
        /// What ended an intercepted TLS handshake: `client_closed`,
        /// `unknown_ca_alert`, `other_alert:<name>`, `timeout` or `other`.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The Proxy forwarded a request to a credential host without the credential.
    CredentialWithheld {
        /// The credential host the request was for.
        host: String,
        /// The tool rule the host belongs to.
        rule: String,
        /// The HTTP method of the request.
        method: String,
        /// A templated allow entry such as `GET /repos/{repo}/pulls/*` that would let
        /// the request through; never a raw path, query, header, body or token.
        shape: Option<String>,
    },
    /// The Proxy could not obtain the credential of a tool rule.
    ProxyCredentialFailed {
        /// The tool rule whose credential failed.
        rule: String,
    },
    /// An upstream answered a credential rule's request with 429: the account behind the rule is rate limited.
    AgentRateLimited {
        /// The credential rule whose account is rate limited.
        rule: String,
        /// The HTTP status of the upstream answer.
        status: u16,
        /// When the limit resets, if the upstream said so.
        resets_at: Option<String>,
    },
    /// An upstream rejected a credential rule's credential with 401 or 403.
    ProxyCredentialRejected {
        /// The credential rule whose credential was rejected.
        rule: String,
        /// The HTTP status of the upstream answer.
        status: u16,
    },
    /// A credential rule's upstream answered 2xx again after a rate limit or rejection.
    ProxyCredentialRecovered {
        /// The credential rule that works again.
        rule: String,
    },
    /// The agent finished the turn a steer started.
    TurnDone {
        /// The steer's id.
        steer: i64,
        /// How the turn ended.
        outcome: TurnOutcome,
    },
    /// A Box file was stored as a new artifact version.
    ArtifactPublished {
        /// The file name.
        name: String,
        /// The version number, starting at 1.
        version: i64,
        /// The artifact kind, such as `screenshot` or `log`.
        artifact_kind: String,
        /// The content type.
        content_type: String,
        /// Size in bytes.
        size: u64,
        /// Lowercase hex SHA-256 of the contents.
        sha256: String,
    },
}

pub use boxes::box_events;
pub use checked::CheckedCred;
pub use steps::{lifecycle_events, session_events};

mod boxes;
mod checked;
mod kind;
mod steps;

#[cfg(test)]
mod proxy_tests;
#[cfg(test)]
mod relaunch_tests;
#[cfg(test)]
mod tests;
