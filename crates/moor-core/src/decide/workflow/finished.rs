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
    /// Whether the last agent launch failed and no later launch succeeded.
    pub launch_failed: bool,
}

/// Where a started run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunVerdict {
    /// The agent works, or the Box is still coming up or waiting for a slot.
    Running,
    /// The agent is done, or the Box is closed.
    Finished,
    /// No agent can run in the Box until a person acts, so the run will never finish.
    Broken,
}

/// Where the run whose Box has `facts`, or none when the Box is gone, stands at `now`.
///
/// It is Finished when the Box is gone or closed, when its agent exited, or when its Session has
/// been Idle for [`QUIET_BEFORE_FINISHED`]. It is Broken when setup failed or the agent launch
/// failed, since only a person retrying starts an agent then. Otherwise it is Running, which
/// includes a Box queued for a slot, one still setting up, and an agent that ended its turn to
/// wait on background work.
#[must_use]
pub fn run_verdict(facts: Option<&RunBoxFacts>, now: Ts) -> RunVerdict {
    use RunVerdict::{Broken, Finished, Running};
    let Some(f) = facts else { return Finished };
    match f.lifecycle {
        BoxLifecycle::Closed | BoxLifecycle::Rejected => return Finished,
        BoxLifecycle::SetupFailed => return Broken,
        BoxLifecycle::Checking | BoxLifecycle::Pending | BoxLifecycle::SettingUp => {
            return Running;
        }
        BoxLifecycle::Ready | BoxLifecycle::Asleep => {}
    }
    if f.launch_failed {
        return Broken;
    }
    let quiet_long_enough = |since: Ts| {
        let quiet = now.as_millis().saturating_sub(since.as_millis());
        u128::try_from(quiet).is_ok_and(|q| q >= QUIET_BEFORE_FINISHED.as_millis())
    };
    match f.session {
        SessionState::Ended => Finished,
        SessionState::Idle if f.idle_since.is_some_and(quiet_long_enough) => Finished,
        SessionState::Idle | SessionState::Starting | SessionState::Working => Running,
    }
}

#[cfg(test)]
mod tests;
