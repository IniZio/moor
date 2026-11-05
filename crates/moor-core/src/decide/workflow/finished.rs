//! When a started workflow run is over.

use std::time::Duration;

use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;

/// How long the agent must have been quiet before its run counts as finished. The agent goes
/// quiet between the turns of one job too, so a single quiet reading is not an end.
pub const QUIET_BEFORE_FINISHED: Duration = Duration::from_secs(120);

/// What the store says of the Box a run started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBoxFacts {
    /// The Box lifecycle.
    pub lifecycle: BoxLifecycle,
    /// The state of the primary Session. An agent waiting on background work is still Working.
    pub session: SessionState,
    /// When the Box last became Idle; none while it is not.
    pub idle_since: Option<Ts>,
}

/// Whether the run whose Box has `facts`, or none when the Box is gone, is finished at `now`.
///
/// It is when the Box is gone or closed, when its agent exited, or when its Session has been
/// Idle for [`QUIET_BEFORE_FINISHED`]. A Session still Starting or Working is not, which includes
/// an agent that ended its turn to wait on background work.
#[must_use]
pub fn run_finished(facts: Option<&RunBoxFacts>, now: Ts) -> bool {
    let Some(f) = facts else { return true };
    if matches!(f.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected) {
        return true;
    }
    match f.session {
        SessionState::Ended => matches!(f.lifecycle, BoxLifecycle::Ready | BoxLifecycle::Asleep),
        SessionState::Idle => f.idle_since.is_some_and(|since| {
            let quiet = now.as_millis().saturating_sub(since.as_millis());
            u128::try_from(quiet).is_ok_and(|q| q >= QUIET_BEFORE_FINISHED.as_millis())
        }),
        SessionState::Starting | SessionState::Working => false,
    }
}

#[cfg(test)]
mod tests;
