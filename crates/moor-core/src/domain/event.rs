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
}

impl Event {
    /// The `snake_case` kind name; equals the serde tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Rejected { .. } => "rejected",
            Self::WaitingForBox { .. } => "waiting_for_box",
            Self::SettingUp { .. } => "setting_up",
            Self::Working => "working",
            Self::SetupFailed { .. } => "setup_failed",
            Self::CredentialFailed { .. } => "credential_failed",
            Self::Idled => "idled",
            Self::Busy => "busy",
            Self::Asleep { .. } => "asleep",
            Self::Woke => "woke",
            Self::Closed { .. } => "closed",
            Self::BoxRequested => "box_requested",
            Self::BoxReady => "box_ready",
            Self::BoxFailed { .. } => "box_failed",
            Self::BoxAsleep => "box_asleep",
            Self::BoxGone => "box_gone",
            Self::BoxDeleted => "box_deleted",
            Self::BoxReleaseFailed { .. } => "box_release_failed",
            Self::StaysUp { .. } => "stays_up",
            Self::Ran { .. } => "ran",
            Self::Copied { .. } => "copied",
            Self::SessionStarted { .. } => "session_started",
            Self::SessionFailed { .. } => "session_failed",
            Self::Relaunched { .. } => "relaunched",
            Self::Checked { .. } => "checked",
            Self::RunStarted { .. } => "run_started",
            Self::Attached => "attached",
            Self::SessionsStopped { .. } => "sessions_stopped",
        }
    }

    /// The event fields as a JSON object without the `kind` tag; `{}` for unit variants.
    #[must_use]
    pub fn payload(&self) -> serde_json::Value {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(mut map)) => {
                map.remove("kind");
                serde_json::Value::Object(map)
            }
            _ => serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Whether the event tests pin this kind.
    #[must_use]
    pub fn is_asserted(&self) -> bool {
        ASSERTED_KINDS.contains(&self.kind())
    }
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

#[cfg(test)]
mod tests;
