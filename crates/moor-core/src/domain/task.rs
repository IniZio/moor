//! The Task state machine: every state change is `step()`, and every other pair is an error.

use super::Illegal;

/// Where a Task is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Credential preflight is running; transient.
    Checking,
    /// Preflight failed; terminal, no Box.
    Rejected,
    /// Admitted but the Box capacity is full.
    WaitingForBox,
    /// A Box is being acquired and set up.
    SettingUp,
    /// Acquire, credential or dotfiles setup failed.
    SetupFailed,
    /// The agent is working.
    Working,
    /// The agent is quiet.
    Idle,
    /// The Task is asleep.
    Asleep,
    /// The Task is closed; terminal.
    Closed,
}

impl TaskState {
    /// Every state, in declaration order.
    pub const ALL: [Self; 9] = [
        Self::Checking,
        Self::Rejected,
        Self::WaitingForBox,
        Self::SettingUp,
        Self::SetupFailed,
        Self::Working,
        Self::Idle,
        Self::Asleep,
        Self::Closed,
    ];

    /// The `snake_case` name, identical to the serde form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::Rejected => "rejected",
            Self::WaitingForBox => "waiting_for_box",
            Self::SettingUp => "setting_up",
            Self::SetupFailed => "setup_failed",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Asleep => "asleep",
            Self::Closed => "closed",
        }
    }

    /// Parses a `snake_case` state name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == s)
    }

    /// True for every state but `Closed` and `Rejected`.
    #[must_use]
    pub const fn is_live(self) -> bool {
        !matches!(self, Self::Closed | Self::Rejected)
    }

    /// The state of a new Task; only `Insert` creates one.
    ///
    /// # Errors
    /// Returns [`Illegal`] with `from` = `none` for any input but `Insert`.
    pub fn initial(input: &TaskInput) -> Result<Self, Illegal> {
        match input {
            TaskInput::Insert => Ok(Self::Checking),
            other => Err(Illegal {
                from: "none".to_owned(),
                input: other.name().to_owned(),
            }),
        }
    }

    /// The state after `input`.
    ///
    /// # Errors
    /// Returns [`Illegal`] for any pair outside the transition table.
    pub fn step(self, input: &TaskInput) -> Result<Self, Illegal> {
        use TaskInput as I;
        let to = match (self, input) {
            (Self::Checking, I::Rejected { .. }) => Self::Rejected,
            (Self::Checking, I::AtCapacity { .. }) => Self::WaitingForBox,
            (Self::Checking, I::Admitted)
            | (Self::WaitingForBox, I::Promoted)
            | (Self::SetupFailed, I::Retry) => Self::SettingUp,
            (Self::SettingUp, I::BoxReady { .. })
            | (Self::Idle, I::AgentBusy | I::RunStarted)
            | (Self::Asleep, I::Wake) => Self::Working,
            (
                Self::SettingUp,
                I::AcquireFailed { .. } | I::CredFailed { .. } | I::DotfilesFailed { .. },
            ) => Self::SetupFailed,
            (Self::Working, I::AgentQuiet) => Self::Idle,
            (Self::Idle, I::SleepDue | I::SleepCmd) => Self::Asleep,
            (state, I::Close) if state.is_live() => Self::Closed,
            _ => {
                return Err(Illegal {
                    from: self.as_str().to_owned(),
                    input: input.name().to_owned(),
                });
            }
        };
        Ok(to)
    }
}

/// How a Box's setup script ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOutcome {
    /// Setup passed.
    Ok,
    /// Setup failed; the Task still works, with a warning.
    Failed {
        /// The failing output line.
        line: String,
    },
}

/// A fact that may move a Task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskInput {
    /// A new Task is created.
    Insert,
    /// Preflight rejected the Task.
    Rejected {
        /// Why.
        reason: String,
    },
    /// No Box is free.
    AtCapacity {
        /// The configured Box limit.
        max_boxes: u32,
    },
    /// A Box slot is free.
    Admitted,
    /// A waiting Task got a slot.
    Promoted,
    /// The Box is ready.
    BoxReady {
        /// How setup ended.
        setup: SetupOutcome,
    },
    /// The Driver could not acquire a Box.
    AcquireFailed {
        /// The Driver's cause.
        cause: String,
    },
    /// Credential delivery failed.
    CredFailed {
        /// Which credential.
        which: String,
        /// The error text.
        error: String,
    },
    /// Dotfiles setup failed.
    DotfilesFailed {
        /// The error text.
        error: String,
    },
    /// Setup is retried.
    Retry,
    /// The agent went quiet.
    AgentQuiet,
    /// The agent became busy.
    AgentBusy,
    /// A Run started.
    RunStarted,
    /// The auto-sleep age passed.
    SleepDue,
    /// The user asked to sleep.
    SleepCmd,
    /// The Task wakes.
    Wake,
    /// The Task closes.
    Close,
}

impl TaskInput {
    /// The `snake_case` variant name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Rejected { .. } => "rejected",
            Self::AtCapacity { .. } => "at_capacity",
            Self::Admitted => "admitted",
            Self::Promoted => "promoted",
            Self::BoxReady { .. } => "box_ready",
            Self::AcquireFailed { .. } => "acquire_failed",
            Self::CredFailed { .. } => "cred_failed",
            Self::DotfilesFailed { .. } => "dotfiles_failed",
            Self::Retry => "retry",
            Self::AgentQuiet => "agent_quiet",
            Self::AgentBusy => "agent_busy",
            Self::RunStarted => "run_started",
            Self::SleepDue => "sleep_due",
            Self::SleepCmd => "sleep_cmd",
            Self::Wake => "wake",
            Self::Close => "close",
        }
    }
}

#[cfg(test)]
mod tests;
