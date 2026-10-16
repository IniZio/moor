//! Typed task events. State events are derived from steps, so a row and its event cannot disagree.

use serde::Serialize;

use crate::domain::boxes::{BoxInput, BoxState};
use crate::domain::task::{SetupOutcome, TaskInput, TaskState};
use crate::name::{BoxHandle, RunId};

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

/// One credential an admission check looked at; never carries a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckedCred {
    /// The credential name.
    pub name: String,
    /// The config layer that defines it.
    pub source: String,
    /// Whether it resolved.
    pub resolves: bool,
}

/// One recorded fact about a task, serialized with its `kind` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// The task row was created.
    Started,
    /// Admission refused the task.
    Rejected {
        /// Why the task was refused.
        reason: String,
    },
    /// The task waits for a free box slot.
    WaitingForBox {
        /// The capacity limit that was hit.
        max_boxes: u32,
    },
    /// Box setup began.
    SettingUp {
        /// The state the task left.
        from: TaskState,
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
    /// A per-Task device could not be provisioned; the Task launched without it.
    DeviceProvisionFailed {
        /// The device kind, such as `ios`.
        device: String,
        /// Why provisioning failed.
        reason: String,
    },
    /// The agent went quiet.
    Idled,
    /// The agent became busy again.
    Busy,
    /// The task went to sleep.
    Asleep {
        /// True when the sleep timer fired, false for an explicit command.
        auto: bool,
    },
    /// The task woke.
    Woke,
    /// The task closed.
    Closed {
        /// The state the task left.
        from: TaskState,
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
    /// The driver refused to sleep an idle task's box, so it keeps running.
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
    /// A start attached to an existing open task.
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
    /// A message or menu choice was sent to the agent.
    Steered {
        /// The agent session it went to.
        session: String,
        /// What was sent.
        message: String,
        /// Whether the submit key was pressed a second time.
        submit_retried: bool,
    },
    /// The Proxy for the task is ready to accept connections.
    ProxyStarted {
        /// Port the Box side connects to.
        box_port: u16,
        /// Port the Proxy listens on, on the host.
        host_port: u16,
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
    /// The Task's Proxy refused a connection to a host not allowed for the Task.
    EgressRefused {
        /// The host the connection was for.
        host: String,
        /// The tool rule the host belongs to, when it belongs to one.
        rule: Option<String>,
        /// The HTTP method of the refused request, when one was seen.
        method: Option<String>,
        /// Why the Proxy refused the connection, as the Proxy names it.
        category: Option<String>,
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
}

/// The events a task step produces.
///
/// `from` is `None` for the initial insert; `Closed` then reports `to` as its origin.
#[must_use]
pub fn task_events(from: Option<TaskState>, to: TaskState, input: &TaskInput) -> Vec<Event> {
    let origin = from.unwrap_or(to);
    match input {
        TaskInput::Insert => vec![Event::Started],
        TaskInput::Rejected { reason } => vec![Event::Rejected {
            reason: reason.clone(),
        }],
        TaskInput::AtCapacity { max_boxes } => vec![Event::WaitingForBox {
            max_boxes: *max_boxes,
        }],
        TaskInput::Admitted | TaskInput::Promoted | TaskInput::Retry => {
            vec![Event::SettingUp { from: origin }]
        }
        TaskInput::BoxReady { setup } => match setup {
            SetupOutcome::Ok => vec![Event::Working],
            SetupOutcome::Failed { line } => {
                vec![Event::Working, Event::SetupFailed { line: line.clone() }]
            }
        },
        TaskInput::AcquireFailed { cause } => vec![Event::SetupFailed {
            line: cause.clone(),
        }],
        TaskInput::DotfilesFailed { error } => vec![Event::SetupFailed {
            line: error.clone(),
        }],
        TaskInput::CredFailed { which, error } => vec![
            Event::CredentialFailed {
                which: which.clone(),
                error: error.clone(),
            },
            Event::SetupFailed {
                line: error.clone(),
            },
        ],
        TaskInput::AgentQuiet => vec![Event::Idled],
        TaskInput::AgentBusy | TaskInput::RunStarted => vec![Event::Busy],
        TaskInput::SleepDue => vec![Event::Asleep { auto: true }],
        TaskInput::SleepCmd => vec![Event::Asleep { auto: false }],
        TaskInput::Wake => vec![Event::Woke],
        TaskInput::Close => vec![Event::Closed { from: origin }],
    }
}

/// The events a box step produces; `handle` is the Box row's current handle.
#[must_use]
pub fn box_events(
    _from: Option<BoxState>,
    _to: BoxState,
    input: &BoxInput,
    handle: Option<&BoxHandle>,
) -> Vec<Event> {
    let text = |h: Option<&BoxHandle>| h.map(|h| h.as_str().to_owned());
    match input {
        BoxInput::Requested => vec![Event::BoxRequested],
        BoxInput::Acquired { .. } | BoxInput::Wake => vec![Event::BoxReady],
        BoxInput::AcquireFailed { cause, attempted } => vec![Event::BoxFailed {
            error: cause.clone(),
            handle: text(attempted.as_ref().or(handle)),
        }],
        BoxInput::Sleep => vec![Event::BoxAsleep],
        BoxInput::DriverMissing => vec![Event::BoxGone],
        BoxInput::Released => vec![Event::BoxDeleted],
        BoxInput::ReleaseErr { cause } => vec![Event::BoxReleaseFailed {
            error: cause.clone(),
            handle: text(handle),
        }],
    }
}

mod kind;

#[cfg(test)]
mod proxy_tests;
#[cfg(test)]
mod tests;
