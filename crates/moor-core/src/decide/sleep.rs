//! Auto-sleep verdicts.

use std::time::Duration;

use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;

/// What auto-sleep can observe about a Task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SleepFacts {
    /// The Box lifecycle state.
    pub lifecycle: BoxLifecycle,
    /// The state of the primary Session.
    pub session: SessionState,
    /// When the Task last became Idle, if known.
    pub idle_since: Option<Ts>,
    /// The current time.
    pub now: Ts,
    /// How long a Task must stay Idle before it sleeps.
    pub after: Duration,
}

/// Outcome of one auto-sleep check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepVerdict {
    /// The Task has been Idle long enough to sleep.
    Due,
    /// The Task is Idle but not for long enough, or its idle time is unknown.
    NotYet,
    /// The Task is not Idle.
    NotIdle,
}

/// Decides auto-sleep: only an Idle Task is due, once `now - idle_since >= after`.
/// A clock that went backwards is never due.
#[must_use]
pub fn sleep_due(f: &SleepFacts) -> SleepVerdict {
    if !(f.lifecycle == BoxLifecycle::Ready && f.session.is_quiet()) {
        return SleepVerdict::NotIdle;
    }
    let Some(since) = f.idle_since else {
        return SleepVerdict::NotYet;
    };
    let elapsed_ms = i128::from(f.now.as_millis()) - i128::from(since.as_millis());
    if elapsed_ms >= 0 && elapsed_ms.unsigned_abs() >= f.after.as_millis() {
        SleepVerdict::Due
    } else {
        SleepVerdict::NotYet
    }
}

#[cfg(test)]
mod tests;
