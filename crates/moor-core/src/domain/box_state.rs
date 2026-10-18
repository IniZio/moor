//! The Box lifecycle machine: every state change is `step()`, and every other pair is an error.
//!
//! This will replace both the Box half of `TaskState` and `boxes::BoxState` when the schema
//! changes over. Until then nothing uses it.

use super::session_state::SessionState;
use super::task::{SetupOutcome, TaskState};
use super::Illegal;

/// Why a Box lifecycle step was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StepError {
    /// The pair is outside the transition table.
    #[error(transparent)]
    Illegal(#[from] Illegal),
    /// The Box is Ready, but a live Session is not Idle.
    #[error("sleep refused: {busy} Session(s) not idle")]
    SleepBlocked {
        /// How many live Sessions are not Idle.
        busy: usize,
    },
}

/// Where a Box is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxLifecycle {
    /// Credential preflight is running; transient.
    Checking,
    /// Preflight failed; terminal, no machine.
    Rejected,
    /// Admitted but the Box capacity is full.
    WaitingForBox,
    /// The machine is being acquired and set up.
    SettingUp,
    /// Acquire, credential, dotfiles or sync setup failed.
    SetupFailed,
    /// The machine is up and can host Sessions.
    Ready,
    /// The machine sleeps; its Sessions are suspended, never ended.
    Asleep,
    /// The Box is closed; terminal.
    Closed,
}

impl BoxLifecycle {
    /// Every state, in declaration order.
    pub const ALL: [Self; 8] = [
        Self::Checking,
        Self::Rejected,
        Self::WaitingForBox,
        Self::SettingUp,
        Self::SetupFailed,
        Self::Ready,
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
            Self::Ready => "ready",
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

    /// The state of a new Box; only `Insert` creates one.
    ///
    /// # Errors
    /// Returns [`Illegal`] with `from` = `none` for any input but `Insert`.
    pub fn initial(input: &BoxLifecycleInput) -> Result<Self, Illegal> {
        match input {
            BoxLifecycleInput::Insert => Ok(Self::Checking),
            other => Err(Illegal {
                from: "none".to_owned(),
                input: other.name().to_owned(),
            }),
        }
    }

    /// The state after `input`.
    ///
    /// # Errors
    /// Returns [`StepError::Illegal`] for any pair outside the transition table, and
    /// [`StepError::SleepBlocked`] for `Sleep` from Ready while a live Session is not Idle.
    pub fn step(self, input: &BoxLifecycleInput) -> Result<Self, StepError> {
        use BoxLifecycleInput as I;
        let to = match (self, input) {
            (Self::Checking, I::Rejected { .. }) => Self::Rejected,
            (Self::Checking, I::AtCapacity { .. }) => Self::WaitingForBox,
            (Self::Checking, I::Admitted)
            | (Self::WaitingForBox, I::Promoted)
            | (Self::SetupFailed, I::Retry) => Self::SettingUp,
            (Self::SettingUp, I::Ready { .. }) | (Self::Asleep, I::Wake) => Self::Ready,
            (Self::SettingUp, I::SetupFailed { .. }) => Self::SetupFailed,
            (Self::Ready, I::Sleep { sessions }) => {
                if !box_may_sleep(sessions) {
                    let busy = sessions
                        .iter()
                        .filter(|s| !matches!(s, SessionState::Idle | SessionState::Ended))
                        .count();
                    return Err(StepError::SleepBlocked { busy });
                }
                Self::Asleep
            }
            (state, I::Close) if state.is_live() => Self::Closed,
            _ => {
                return Err(StepError::Illegal(Illegal {
                    from: self.as_str().to_owned(),
                    input: input.name().to_owned(),
                }));
            }
        };
        Ok(to)
    }
}

/// A fact that may move a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxLifecycleInput {
    /// A new Box is created.
    Insert,
    /// Preflight rejected the Box.
    Rejected {
        /// Why.
        reason: String,
    },
    /// No machine slot is free.
    AtCapacity {
        /// The configured Box limit.
        max_boxes: u32,
    },
    /// A machine slot is free.
    Admitted,
    /// A waiting Box got a slot.
    Promoted,
    /// The machine is ready.
    Ready {
        /// How setup ended; a failed setup still leaves the Box Ready, with a warning.
        setup: SetupOutcome,
    },
    /// Acquire, credential, dotfiles or sync setup failed.
    SetupFailed {
        /// The failure cause.
        cause: String,
        /// The credential that failed, when the cause is a credential.
        which: Option<String>,
    },
    /// Setup is retried.
    Retry,
    /// The Box sleeps, by age or by command; legal only when every Session is Idle.
    Sleep {
        /// The states of the Box's Sessions.
        sessions: Vec<SessionState>,
    },
    /// The Box wakes.
    Wake,
    /// The Box closes.
    Close,
}

impl BoxLifecycleInput {
    /// The `snake_case` variant name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Rejected { .. } => "rejected",
            Self::AtCapacity { .. } => "at_capacity",
            Self::Admitted => "admitted",
            Self::Promoted => "promoted",
            Self::Ready { .. } => "ready",
            Self::SetupFailed { .. } => "setup_failed",
            Self::Retry => "retry",
            Self::Sleep { .. } => "sleep",
            Self::Wake => "wake",
            Self::Close => "close",
        }
    }
}

/// True when a Box may sleep: every Session that has not ended is Idle.
#[must_use]
pub fn box_may_sleep(sessions: &[SessionState]) -> bool {
    sessions
        .iter()
        .all(|s| matches!(s, SessionState::Idle | SessionState::Ended))
}

/// The Box and Session states that together stand for one Task state.
#[must_use]
pub const fn split_task_state(state: TaskState) -> (BoxLifecycle, Option<SessionState>) {
    match state {
        TaskState::Checking => (BoxLifecycle::Checking, None),
        TaskState::Rejected => (BoxLifecycle::Rejected, None),
        TaskState::WaitingForBox => (BoxLifecycle::WaitingForBox, None),
        TaskState::SettingUp => (BoxLifecycle::SettingUp, None),
        TaskState::SetupFailed => (BoxLifecycle::SetupFailed, None),
        TaskState::Working => (BoxLifecycle::Ready, Some(SessionState::Working)),
        TaskState::Idle => (BoxLifecycle::Ready, Some(SessionState::Idle)),
        TaskState::Asleep => (BoxLifecycle::Asleep, Some(SessionState::Idle)),
        TaskState::Closed => (BoxLifecycle::Closed, Some(SessionState::Ended)),
    }
}

#[cfg(test)]
mod tests;
