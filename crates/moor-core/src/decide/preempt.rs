//! Making room on a full node by suspending its longest-idle Boxes.

use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::name::BoxId;
use crate::seam::clock::Ts;

/// A Box on the node, as the scheduler sees it when the node is short of memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resident {
    /// The Box.
    pub id: BoxId,
    /// Its lifecycle.
    pub lifecycle: BoxLifecycle,
    /// Its primary Session.
    pub session: SessionState,
    /// Whether its agent waits on a question.
    pub needs_input: bool,
    /// When it last became Idle.
    pub idle_since: Option<Ts>,
}

/// Whether suspending `r` cannot lose work: it is up and its agent waits on nothing.
#[must_use]
pub fn suspendable(r: &Resident) -> bool {
    r.lifecycle == BoxLifecycle::Ready
        && r.session == SessionState::Idle
        && !r.needs_input
        && r.idle_since.is_some()
}

/// The Box that has been idle longest among those whose suspension cannot lose work.
#[must_use]
pub fn longest_idle(residents: &[Resident]) -> Option<BoxId> {
    residents
        .iter()
        .filter(|r| suspendable(r))
        .min_by_key(|r| (r.idle_since.map(Ts::as_millis), r.id))
        .map(|r| r.id)
}

#[cfg(test)]
mod tests;
