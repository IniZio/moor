//! The Box state machine: every illegal transition is an error.

use super::Illegal;
use crate::name::BoxHandle;

/// Which acquire of a Box's hands a row records; a later acquire has a higher one.
///
/// A write names the incarnation it was decided against, and the store refuses it once the Box's
/// current hands are another, so a late write about a lost VM never lands on its replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HandsInc(i64);

impl HandsInc {
    /// A Box's first hands.
    pub const FIRST: Self = Self(1);

    /// The incarnation stored as `inc`.
    #[must_use]
    pub const fn new(inc: i64) -> Self {
        Self(inc)
    }

    /// The stored `inc`.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// The hands after these.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// The hands an acquire writes, given the Box's `current` ones: new hands after ones that
    /// ended, the current ones while they are still being acquired, the first for a Box with none.
    #[must_use]
    pub const fn acquiring(current: Option<(Self, HandsState)>) -> Self {
        match current {
            None => Self::FIRST,
            Some((inc, state)) if state.replaceable() => inc.next(),
            Some((inc, _)) => inc,
        }
    }
}

/// Lifecycle state of a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandsState {
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

impl HandsState {
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

    /// Whether hands in this state are over: their acquire failed, their VM was lost or released.
    /// Nothing acquires them again; an acquire gets the Box new hands.
    #[must_use]
    pub const fn ended(self) -> bool {
        matches!(self, Self::Failed | Self::Gone | Self::Deleted)
    }

    /// Whether a Box whose current hands are in this state may acquire new ones.
    #[must_use]
    pub const fn replaceable(self) -> bool {
        matches!(self, Self::Failed | Self::Gone)
    }

    /// The state of new hands; only `Requested` creates them.
    ///
    /// # Errors
    /// [`Illegal`] with `from: "none"` for any other input.
    pub fn initial(input: &HandsInput) -> Result<Self, Illegal> {
        match input {
            HandsInput::Requested => Ok(Self::Starting),
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
    pub fn step(self, input: &HandsInput) -> Result<Self, Illegal> {
        use HandsInput as I;
        let to = match (self, input) {
            (Self::Starting, I::Acquired { .. }) | (Self::Asleep, I::Wake) => Self::Ready,
            (Self::Starting, I::AcquireFailed { .. } | I::AcquireCut { .. }) => Self::Failed,
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
pub enum HandsInput {
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
    /// A controller restart cut the acquire off and the start acquires again, so the Box has not
    /// failed.
    AcquireCut {
        /// A handle that may have been created before the cut.
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

impl HandsInput {
    /// The `snake_case` variant name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Acquired { .. } => "acquired",
            Self::AcquireFailed { .. } => "acquire_failed",
            Self::AcquireCut { .. } => "acquire_cut",
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
