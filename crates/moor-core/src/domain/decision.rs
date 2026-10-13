//! The single value a store writer applies: inputs, field changes, intent and events.

use super::boxes::BoxInput;
use super::event::{CheckedCred, Event};
use super::task::TaskInput;
use crate::name::{BoxHandle, ProviderId, RunId, TaskId, TaskName};

/// A change to one optional column.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Field<T> {
    /// Leave the column as it is.
    #[default]
    Keep,
    /// Write this value.
    Set(T),
    /// Write NULL.
    Clear,
}

/// Column changes for a task and its box row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fields {
    /// Issue reference.
    pub issue: Field<String>,
    /// Task goal.
    pub goal: Field<String>,
    /// Base ref.
    pub base: Field<String>,
    /// Branch name.
    pub branch: Field<String>,
    /// Worktree path.
    pub worktree: Field<String>,
    /// Whether the worktree is bound.
    pub bound: Field<bool>,
    /// Surface workspace id.
    pub workspace_id: Field<String>,
    /// Agent session id.
    pub agent_session_id: Field<String>,
    /// Reason for the current state.
    pub state_reason: Field<String>,
    /// When setup failed, in milliseconds since the epoch.
    pub setup_failed_at: Field<i64>,
    /// The recorded launch failure, as JSON.
    pub launch_failure: Field<String>,
    /// Box provider.
    pub box_provider: Field<ProviderId>,
    /// Box handle.
    pub box_handle: Field<BoxHandle>,
    /// Box workdir.
    pub box_workdir: Field<String>,
    /// Whether egress is enforced in the box.
    pub egress_enforced: Field<bool>,
    /// Why the box cannot sleep, as last reported by the driver.
    pub box_sleep_reason: Field<String>,
}

/// A recorded in-flight operation, kept so a crash can be recovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// A box is being provisioned by this process.
    Provisioning {
        /// Process id.
        pid: u32,
    },
    /// A run is executing.
    Running {
        /// The run.
        run: RunId,
        /// Process id.
        pid: u32,
    },
    /// Box setup is executing.
    Setup {
        /// Process id.
        pid: u32,
    },
}

impl Intent {
    /// Returns the stored kind name.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Provisioning { .. } => "provisioning",
            Self::Running { .. } => "running",
            Self::Setup { .. } => "setup",
        }
    }

    /// Returns the owning process id.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        match self {
            Self::Provisioning { pid } | Self::Running { pid, .. } | Self::Setup { pid } => *pid,
        }
    }

    /// Returns the run for a running intent.
    #[must_use]
    pub const fn run(&self) -> Option<RunId> {
        match self {
            Self::Running { run, .. } => Some(*run),
            Self::Provisioning { .. } | Self::Setup { .. } => None,
        }
    }
}

/// A change to the task intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntentChange {
    /// Leave the intent as it is.
    #[default]
    Keep,
    /// Record this intent.
    Set(Intent),
    /// Remove the intent.
    Clear,
}

/// A task that does not exist yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    /// Task name.
    pub name: TaskName,
    /// Repo the task works on.
    pub repo: String,
}

/// The task a decision applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskTarget {
    /// Insert this task.
    New(NewTask),
    /// Update this task.
    Existing(TaskId),
}

/// An event a caller may pass; every other kind is derived from state steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraEvent {
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
    /// The agent launch failed; the Task stays in its state, so no step derives it.
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
    /// Setup failed on a Task that stays in its state, so no step derives it.
    SetupFailed {
        /// The failing output line.
        line: String,
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

impl From<ExtraEvent> for Event {
    fn from(e: ExtraEvent) -> Self {
        match e {
            ExtraEvent::StaysUp { reason } => Self::StaysUp { reason },
            ExtraEvent::Ran {
                run,
                exit,
                interrupted,
            } => Self::Ran {
                run,
                exit,
                interrupted,
            },
            ExtraEvent::Copied { from, to } => Self::Copied { from, to },
            ExtraEvent::SessionStarted { session } => Self::SessionStarted { session },
            ExtraEvent::SessionFailed { step, message } => Self::SessionFailed { step, message },
            ExtraEvent::Relaunched {
                old_pane,
                new_pane,
                goal,
            } => Self::Relaunched {
                old_pane,
                new_pane,
                goal,
            },
            ExtraEvent::SetupFailed { line } => Self::SetupFailed { line },
            ExtraEvent::Checked { credentials } => Self::Checked { credentials },
            ExtraEvent::RunStarted { run } => Self::RunStarted { run },
            ExtraEvent::Attached => Self::Attached,
            ExtraEvent::SessionsStopped { sessions } => Self::SessionsStopped { sessions },
        }
    }
}

impl TryFrom<Event> for ExtraEvent {
    type Error = Event;

    /// Returns the original event when its kind is derived.
    fn try_from(e: Event) -> Result<Self, Event> {
        match e {
            Event::StaysUp { reason } => Ok(Self::StaysUp { reason }),
            Event::Ran {
                run,
                exit,
                interrupted,
            } => Ok(Self::Ran {
                run,
                exit,
                interrupted,
            }),
            Event::Copied { from, to } => Ok(Self::Copied { from, to }),
            Event::SessionStarted { session } => Ok(Self::SessionStarted { session }),
            Event::SessionFailed { step, message } => Ok(Self::SessionFailed { step, message }),
            Event::Relaunched {
                old_pane,
                new_pane,
                goal,
            } => Ok(Self::Relaunched {
                old_pane,
                new_pane,
                goal,
            }),
            Event::SetupFailed { line } => Ok(Self::SetupFailed { line }),
            Event::Checked { credentials } => Ok(Self::Checked { credentials }),
            Event::RunStarted { run } => Ok(Self::RunStarted { run }),
            Event::Attached => Ok(Self::Attached),
            Event::SessionsStopped { sessions } => Ok(Self::SessionsStopped { sessions }),
            e @ (Event::Started
            | Event::Rejected { .. }
            | Event::WaitingForBox { .. }
            | Event::SettingUp { .. }
            | Event::Working
            | Event::CredentialFailed { .. }
            | Event::Idled
            | Event::Busy
            | Event::Asleep { .. }
            | Event::Woke
            | Event::Closed { .. }
            | Event::BoxRequested
            | Event::BoxReady
            | Event::BoxFailed { .. }
            | Event::BoxAsleep
            | Event::BoxGone
            | Event::BoxDeleted
            | Event::BoxReleaseFailed { .. }) => Err(e),
        }
    }
}

/// Everything one store write changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The task to write.
    pub task: TaskTarget,
    /// Task state machine input.
    pub task_input: Option<TaskInput>,
    /// Box state machine input.
    pub box_input: Option<BoxInput>,
    /// Column changes.
    pub set: Fields,
    /// Intent change.
    pub intent: IntentChange,
    /// Caller-supplied events appended after the derived ones. Derived state
    /// events cannot be passed here; see [`ExtraEvent`].
    pub events: Vec<ExtraEvent>,
}

impl Decision {
    /// Starts a decision for an existing task.
    #[must_use]
    pub fn new(task: TaskId) -> Self {
        Self::base(TaskTarget::Existing(task), None)
    }

    /// Starts a decision that inserts a task.
    #[must_use]
    pub fn insert(new: NewTask) -> Self {
        Self::base(TaskTarget::New(new), Some(TaskInput::Insert))
    }

    /// Builds a decision with no changes.
    fn base(task: TaskTarget, task_input: Option<TaskInput>) -> Self {
        Self {
            task,
            task_input,
            box_input: None,
            set: Fields::default(),
            intent: IntentChange::Keep,
            events: Vec::new(),
        }
    }

    /// Sets the task input.
    #[must_use]
    pub fn task_input(mut self, i: TaskInput) -> Self {
        self.task_input = Some(i);
        self
    }

    /// Sets the box input.
    #[must_use]
    pub fn box_input(mut self, i: BoxInput) -> Self {
        self.box_input = Some(i);
        self
    }

    /// Sets the column changes.
    #[must_use]
    pub fn set(mut self, f: Fields) -> Self {
        self.set = f;
        self
    }

    /// Sets the intent change.
    #[must_use]
    pub const fn intent(mut self, c: IntentChange) -> Self {
        self.intent = c;
        self
    }

    /// Appends an extra event.
    ///
    /// Derived state events (closed, idled, `box_*` ...) are not accepted:
    ///
    /// ```compile_fail
    /// use moor_core::domain::decision::Decision;
    /// use moor_core::domain::event::Event;
    /// use moor_core::domain::task::TaskState;
    /// use moor_core::name::TaskId;
    ///
    /// let _ = Decision::new(TaskId::new(1)).event(Event::Closed { from: TaskState::Idle });
    /// ```
    #[must_use]
    pub fn event(mut self, e: ExtraEvent) -> Self {
        self.events.push(e);
        self
    }
}

#[cfg(test)]
mod tests;
