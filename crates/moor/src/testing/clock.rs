//! Deterministic clock fake.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use moor_core::seam::clock::{Clock, Ts};

use crate::adapters::clock::SystemClock;

/// Deterministic clock that advances only when `sleep` is called.
#[derive(Debug)]
pub(crate) struct FakeClock {
    /// Total time slept so far.
    offset: Mutex<Duration>,
    /// Starting timestamp.
    start: Ts,
    /// Starting instant.
    origin: Instant,
}

impl FakeClock {
    /// Create a fake clock starting at `start`.
    pub(crate) fn new(start: Ts) -> Self {
        Self {
            offset: Mutex::new(Duration::ZERO),
            start,
            origin: SystemClock::real().instant(),
        }
    }

    /// Time slept so far.
    fn offset(&self) -> Duration {
        *self.offset.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Ts {
        self.start.saturating_add(self.offset())
    }

    fn instant(&self) -> Instant {
        self.origin + self.offset()
    }

    fn sleep(&self, d: Duration) {
        let mut off = self.offset.lock().unwrap_or_else(PoisonError::into_inner);
        *off = off.saturating_add(d);
    }
}

#[cfg(test)]
mod tests;
