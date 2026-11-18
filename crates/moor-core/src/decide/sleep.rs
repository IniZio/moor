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
    /// How long a Box stays awake before it sleeps once it is idle.
    pub max_awake: Duration,
    /// How long any Box stays awake before it sleeps, working or not.
    pub hard_ceiling: Duration,
    /// Whether the agent ended its turn but waits on a background job it started.
    pub background_job: bool,
}

/// Why a Box past `max_awake` is held awake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldReason {
    /// The agent is mid-turn.
    Working,
    /// The agent waits on a background job.
    BackgroundJob,
}

impl HoldReason {
    /// The `snake_case` name recorded in the `sleep_held` event.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::BackgroundJob => "background_job",
        }
    }
}

/// Outcome of one auto-sleep check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepVerdict {
    /// The Box has been Idle long enough, or Idle past `max_awake`, to sleep.
    Due,
    /// The Box has been awake past `hard_ceiling`, so it sleeps whatever its Session does.
    DueAtCeiling,
    /// The Box is past `max_awake` but its agent works, so it stays awake.
    Held(HoldReason),
    /// The Box is Idle but not for long enough, or its idle time is unknown.
    NotYet,
    /// The Box is not Idle.
    NotIdle,
}

/// Decides auto-sleep.
///
/// A Ready Box is due at once when it has been awake for `hard_ceiling`, whatever its Session
/// does. Past `max_awake` an Idle Box is due and a busy one is held. Otherwise only an Idle Box
/// is due, once `now - idle_since` reaches `after`, or the shorter `after_blocked` while it
/// waits on a prompt. A clock that went backwards is never due.
#[must_use]
pub fn sleep_due(f: &SleepFacts) -> SleepVerdict {
    if f.lifecycle != BoxLifecycle::Ready {
        return SleepVerdict::NotIdle;
    }
    if f.awake_since
        .is_some_and(|t| lasted(t, f.now, f.hard_ceiling))
    {
        return SleepVerdict::DueAtCeiling;
    }
    let capped = f.awake_since.is_some_and(|t| lasted(t, f.now, f.max_awake));
    if !f.session.is_quiet() {
        return match (capped, f.background_job) {
            (false, _) => SleepVerdict::NotIdle,
            (true, true) => SleepVerdict::Held(HoldReason::BackgroundJob),
            (true, false) => SleepVerdict::Held(HoldReason::Working),
        };
    }
    if capped {
        return SleepVerdict::Due;
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
