//! The Box state machine: every illegal transition is an error.

use super::Illegal;
use crate::name::BoxHandle;

/// Lifecycle state of a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxState {
    /// Acquire requested, no handle yet.
    Starting,
    /// Acquired and usable.
    Ready,
    /// Acquire failed.
    Failed,
    /// Slept by moor.
    Asleep,
    /// The Driver no longer knows the Box.
    Gone,
    /// A release was attempted and failed.
    ReleaseFailed,
    /// Released.
    Deleted,
}

impl BoxState {
    /// Every state, in declaration order.
    pub const ALL: [Self; 7] = [
        Self::Starting,
        Self::Ready,
        Self::Failed,
        Self::Asleep,
        Self::Gone,
        Self::ReleaseFailed,
        Self::Deleted,
    ];

    /// The `snake_case` name, identical to the serde form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Asleep => "asleep",
            Self::Gone => "gone",
            Self::ReleaseFailed => "release_failed",
            Self::Deleted => "deleted",
        }
    }

    /// Parses a `snake_case` state name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|st| st.as_str() == s)
    }

    /// Whether the Box counts toward `max_boxes`.
    #[must_use]
    pub const fn counts_toward_capacity(self) -> bool {
        matches!(self, Self::Starting | Self::Ready | Self::Asleep)
    }

    /// The state of a Box that has no row yet; only `Requested` creates one.
    ///
    /// # Errors
    /// [`Illegal`] with `from: "none"` for any other input.
    pub fn initial(input: &BoxInput) -> Result<Self, Illegal> {
        match input {
            BoxInput::Requested => Ok(Self::Starting),
            _ => Err(Illegal {
                from: "none".to_owned(),
                input: input.name().to_owned(),
            }),
        }
    }

    /// The state after `input`.
    ///
    /// # Errors
    /// [`Illegal`] for every pair outside the transition table.
    pub fn step(self, input: &BoxInput) -> Result<Self, Illegal> {
        use BoxInput as I;
        let to = match (self, input) {
            (Self::Starting, I::Acquired { .. }) | (Self::Asleep, I::Wake) => Self::Ready,
            (Self::Starting, I::AcquireFailed { .. }) => Self::Failed,
            (Self::Failed | Self::Gone, I::Requested) => Self::Starting,
            (Self::Ready, I::Sleep) => Self::Asleep,
            (Self::Ready | Self::Asleep | Self::Failed, I::DriverMissing) => Self::Gone,
            (
                Self::Starting
                | Self::Ready
                | Self::Asleep
                | Self::Failed
                | Self::Gone
                | Self::ReleaseFailed,
                I::Released,
            ) => Self::Deleted,
            (Self::Ready | Self::Asleep | Self::Failed | Self::Gone, I::ReleaseErr { .. }) => {
                Self::ReleaseFailed
            }
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

/// An input to the Box machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxInput {
    /// A Box is wanted.
    Requested,
    /// The Driver returned a handle.
    Acquired {
        /// The acquired handle.
        handle: BoxHandle,
    },
    /// The Driver failed to acquire.
    AcquireFailed {
        /// Why it failed.
        cause: String,
        /// A handle that may have been created before the failure.
        attempted: Option<BoxHandle>,
    },
    /// Put the Box to sleep.
    Sleep,
    /// Wake a sleeping Box.
    Wake,
    /// The Driver reports the Box missing.
    DriverMissing,
    /// The Box was released.
    Released,
    /// A release failed.
    ReleaseErr {
        /// Why it failed.
        cause: String,
    },
}

impl BoxInput {
    /// The `snake_case` variant name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Acquired { .. } => "acquired",
            Self::AcquireFailed { .. } => "acquire_failed",
            Self::Sleep => "sleep",
            Self::Wake => "wake",
            Self::DriverMissing => "driver_missing",
            Self::Released => "released",
            Self::ReleaseErr { .. } => "release_err",
        }
    }
}

#[cfg(test)]
mod tests;
