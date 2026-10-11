//! Clock seam: the single injectable source of time.

use std::time::{Duration, Instant};

/// A timestamp in unix milliseconds.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct Ts(i64);

impl Ts {
    /// Builds a timestamp from unix milliseconds.
    #[must_use]
    pub const fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    /// Returns the unix milliseconds.
    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// Adds a duration, saturating at the bounds of `i64` milliseconds.
    #[must_use]
    pub fn saturating_add(self, d: Duration) -> Self {
        let ms = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
        Self(self.0.saturating_add(ms))
    }
}

/// Source of wall-clock time, monotonic time and sleeping.
pub trait Clock: Send + Sync {
    /// Current wall-clock time.
    fn now(&self) -> Ts;
    /// Current monotonic instant.
    fn instant(&self) -> Instant;
    /// Blocks (or, in a fake, advances time) for `d`.
    fn sleep(&self, d: Duration);
}

#[cfg(test)]
mod tests;
