//! What a Box's gates add to its state and its line.

use super::Progress;
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::gate::{GateNext, GateStatus};
use crate::view::BoxView;

/// The status of the gates of an open Box; none for a Box without gates or a closed one.
fn open_status(b: &BoxView) -> Option<&GateStatus> {
    b.gate
        .as_ref()
        .map(|g| &g.status)
        .filter(|_| matches!(b.lifecycle, BoxLifecycle::Ready | BoxLifecycle::Asleep))
}

/// The state the gates force on an open Box, even asleep: one that waits for a person needs them,
/// one that failed fails the Box.
pub(super) fn forced(b: &BoxView) -> Option<Progress> {
    match open_status(b)? {
        GateStatus::Stuck { .. }
        | GateStatus::Open {
            next: GateNext::Ask,
            ..
        } => Some(Progress::NeedsYou),
        GateStatus::Failed { .. } => Some(Progress::Failed),
        GateStatus::Open { .. } | GateStatus::Passed => None,
    }
}

/// Whether a gate is still open, so an agent that ended its turn has not finished the Box.
pub(super) fn holds(b: &BoxView) -> bool {
    b.gate
        .as_ref()
        .is_some_and(|g| g.status != GateStatus::Passed)
}

/// The line a gate gives a Box in `state`, when it has something to say that the agent's own
/// words do not.
pub(super) fn line(state: Progress, b: &BoxView) -> Option<String> {
    let status = open_status(b)?;
    let idle = matches!(b.session_state, SessionState::Idle | SessionState::Ended);
    match (state, status) {
        (Progress::Working, GateStatus::Open { gate, next, .. }) if idle => Some(match next {
            GateNext::Agent => format!("gate {gate}: waiting for the agent's evidence"),
            GateNext::RunChecks => format!("gate {gate}: running its checks"),
            GateNext::Ask => format!("gate {gate}: waiting for your approval"),
        }),
        (Progress::NeedsYou, GateStatus::Open { gate, .. }) => {
            Some(format!("gate {gate}: waiting for your approval"))
        }
        (Progress::NeedsYou, GateStatus::Stuck { gate, why }) => {
            Some(format!("gate {gate} is stuck: {why}"))
        }
        (Progress::Failed, GateStatus::Failed { gate, why }) => {
            Some(format!("gate {gate} failed: {why}"))
        }
        _ => None,
    }
}
