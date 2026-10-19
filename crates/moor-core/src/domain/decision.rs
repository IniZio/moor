//! The single value a store writer applies: inputs, field changes, intent and events.

mod extra;

use super::box_state::BoxLifecycleInput;
use super::boxes::BoxInput;
use super::session_state::SessionInput;
use crate::name::{BoxHandle, BoxId, BoxName, ProviderId, RunId};

pub use extra::ExtraEvent;

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
    /// Box egress: `none`, `enforced` or `not_enforced`.
    pub egress: Field<String>,
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
    /// Box name.
    pub name: BoxName,
    /// Repo the task works on.
    pub repo: String,
}

/// The task a decision applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskTarget {
    /// Insert this task.
    New(NewTask),
    /// Update this task.
    Existing(BoxId),
}

/// Everything one store write changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The task to write.
    pub task: TaskTarget,
    /// Box lifecycle input.
    pub lifecycle_input: Option<BoxLifecycleInput>,
    /// Primary Session input.
    pub session_input: Option<SessionInput>,
    /// Box state machine input.
    pub box_input: Option<BoxInput>,
    /// Column changes.
    pub set: Fields,
    /// Intent change.
    pub intent: IntentChange,
    /// Caller-supplied events appended after the derived ones. Derived state
    /// events cannot be passed here; see [`ExtraEvent`].
    pub events: Vec<ExtraEvent>,
    /// Parent Session of the first Session, for a Decision that inserts a task.
    pub parent_session: Option<i64>,
}

impl Decision {
    /// Starts a decision for an existing task.
    #[must_use]
    pub fn new(task: BoxId) -> Self {
        Self::base(TaskTarget::Existing(task))
    }

    /// Starts a decision that inserts a task.
    #[must_use]
    pub fn insert(new: NewTask) -> Self {
        Self::base(TaskTarget::New(new)).lifecycle(BoxLifecycleInput::Insert)
    }

    /// Builds a decision with no changes.
    fn base(task: TaskTarget) -> Self {
        Self {
            task,
            lifecycle_input: None,
            session_input: None,
            box_input: None,
            set: Fields::default(),
            intent: IntentChange::Keep,
            events: Vec::new(),
            parent_session: None,
        }
    }

    /// Sets the Box lifecycle input.
    #[must_use]
    pub fn lifecycle(mut self, i: BoxLifecycleInput) -> Self {
        self.lifecycle_input = Some(i);
        self
    }

    /// Sets the primary Session input.
    #[must_use]
    pub const fn session(mut self, i: SessionInput) -> Self {
        self.session_input = Some(i);
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

    /// Sets the parent Session of the first Session; ignored for an existing task.
    #[must_use]
    pub const fn parent_session(mut self, parent: Option<i64>) -> Self {
        self.parent_session = parent;
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
    /// use moor_core::name::BoxId;
    ///
    /// let _ = Decision::new(BoxId::new(1)).event(Event::Closed { from: TaskState::Idle });
    /// ```
    #[must_use]
    pub fn event(mut self, e: ExtraEvent) -> Self {
        self.events.push(e);
        self
    }
}

#[cfg(test)]
mod tests;
