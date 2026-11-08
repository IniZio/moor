//! Background work an agent waits on: when its wait is overdue for a check.

use super::stall::stalled_for;
use super::{AgentState, Term};
use crate::seam::clock::Ts;

/// What the Box shows about the background work the agent waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitCheck {
    /// Some of it still runs.
    Running,
    /// None of it runs.
    Nothing,
    /// The Box could not tell.
    Unknown,
}

/// Whether an agent in `state` has waited on background work, its terminal quiet since `term`'s
/// last status change, for so long that the Box should be asked whether the work still runs.
#[must_use]
pub fn wait_overdue(state: &AgentState, term: &Term, now: Ts) -> bool {
    matches!(state, AgentState::Waiting { .. })
        && term
            .status_at
            .is_some_and(|at| stalled_for(at, now).is_some())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::stall::STALL_AFTER;
    use super::super::{Life, Status};
    use super::*;

    fn term() -> Term {
        Term {
            life: Life::Live,
            status: Some(Status::Idle),
            status_at: Some(Ts::from_millis(1_000_000)),
            turn: 1,
        }
    }

    fn waiting() -> AgentState {
        AgentState::Waiting {
            on: vec!["gh run watch".to_owned()],
        }
    }

    #[test]
    fn a_wait_is_overdue_only_once_quiet_for_the_stall_bound() {
        let at = Ts::from_millis(1_000_000);
        let just_under = at.saturating_add(STALL_AFTER.saturating_sub(Duration::from_millis(1)));
        assert!(!wait_overdue(&waiting(), &term(), just_under));
        assert!(wait_overdue(
            &waiting(),
            &term(),
            at.saturating_add(STALL_AFTER)
        ));
    }

    #[test]
    fn only_a_wait_is_ever_overdue() {
        let late = Ts::from_millis(1_000_000).saturating_add(STALL_AFTER * 10);
        assert!(!wait_overdue(&AgentState::Idle, &term(), late));
        assert!(!wait_overdue(&AgentState::Working, &term(), late));
    }

    #[test]
    fn a_clock_behind_the_status_is_not_overdue() {
        assert!(!wait_overdue(&waiting(), &term(), Ts::from_millis(500_000)));
    }
}
