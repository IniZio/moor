//! Auto-sleep verdicts.

use std::time::Duration;

use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;

/// What auto-sleep can observe about a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SleepFacts {
    /// The Box lifecycle state.
    pub lifecycle: BoxLifecycle,
    /// The state of the primary Session.
    pub session: SessionState,
    /// When the Box last became Idle, if known.
    pub idle_since: Option<Ts>,
    /// The current time.
    pub now: Ts,
    /// How long a Box must stay Idle before it sleeps.
    pub after: Duration,
    /// Whether the agent waits on an unanswered prompt or a first-run screen.
    pub blocked: bool,
    /// How long a Box waiting on a prompt stays Idle before it sleeps.
    pub after_blocked: Duration,
    /// When the machine last came up, by acquire or wake, if known.
    pub awake_since: Option<Ts>,
    /// How long any Box stays awake before it sleeps, whatever its Session does.
    pub max_awake: Duration,
}

/// Outcome of one auto-sleep check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepVerdict {
    /// The Box has been Idle long enough to sleep.
    Due,
    /// The Box is Idle but not for long enough, or its idle time is unknown.
    NotYet,
    /// The Box is not Idle.
    NotIdle,
}

/// Decides auto-sleep.
///
/// A Ready Box is due once it has been awake for `max_awake`, whatever its Session does. Otherwise only an Idle Box is due, once `now - idle_since` reaches `after`, or
/// the shorter `after_blocked` while it waits on a prompt. A clock that went backwards is never due.
#[must_use]
pub fn sleep_due(f: &SleepFacts) -> SleepVerdict {
    if f.lifecycle != BoxLifecycle::Ready {
        return SleepVerdict::NotIdle;
    }
    if f.awake_since.is_some_and(|t| lasted(t, f.now, f.max_awake)) {
        return SleepVerdict::Due;
    }
    if !f.session.is_quiet() {
        return SleepVerdict::NotIdle;
    }
    let after = if f.blocked {
        f.after.min(f.after_blocked)
    } else {
        f.after
    };
    match f.idle_since {
        Some(since) if lasted(since, f.now, after) => SleepVerdict::Due,
        _ => SleepVerdict::NotYet,
    }
}

/// Whether at least `span` has passed from `since` to `now`.
fn lasted(since: Ts, now: Ts, span: Duration) -> bool {
    let elapsed_ms = i128::from(now.as_millis()) - i128::from(since.as_millis());
    elapsed_ms >= 0 && elapsed_ms.unsigned_abs() >= span.as_millis()
}

#[cfg(test)]
mod tests;
