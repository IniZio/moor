//! The single value a store writer applies: inputs, field changes, intent and events.

use super::boxes::BoxInput;
use super::event::Event;
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
    /// Box provider.
    pub box_provider: Field<ProviderId>,
    /// Box handle.
    pub box_handle: Field<BoxHandle>,
    /// Box workdir.
    pub box_workdir: Field<String>,
    /// Whether egress is enforced in the box.
    pub egress_enforced: Field<bool>,
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
    /// Extra events appended after the derived ones.
    pub events: Vec<Event>,
}

impl Decision {
    /// Starts a decision for an existing task.
    #[must_use]
    pub const fn new(task: TaskId) -> Self {
        Self::base(TaskTarget::Existing(task), None)
    }

    /// Starts a decision that inserts a task.
    #[must_use]
    pub const fn insert(new: NewTask) -> Self {
        Self::base(TaskTarget::New(new), Some(TaskInput::Insert))
    }

    /// Builds a decision with no changes.
    const fn base(task: TaskTarget, task_input: Option<TaskInput>) -> Self {
        Self {
            task,
            task_input,
            box_input: None,
            set: Fields {
                issue: Field::Keep,
                goal: Field::Keep,
                base: Field::Keep,
                branch: Field::Keep,
                worktree: Field::Keep,
                bound: Field::Keep,
                workspace_id: Field::Keep,
                agent_session_id: Field::Keep,
                state_reason: Field::Keep,
                box_provider: Field::Keep,
                box_handle: Field::Keep,
                box_workdir: Field::Keep,
                egress_enforced: Field::Keep,
            },
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
    #[must_use]
    pub fn event(mut self, e: Event) -> Self {
        self.events.push(e);
        self
    }
}

#[cfg(test)]
mod tests;
