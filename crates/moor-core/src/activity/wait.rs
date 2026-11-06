//! Whether an agent's reported wait on background work still stands.

use super::hook::{HookKind, HookRecord};
use super::stall::stalled_for;
use crate::seam::clock::Ts;

/// What the Box shows about the background work a waiting agent named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitCheck {
    /// A process of that work is alive.
    Running,
    /// All of that work could be looked for, and none of it is there.
    Nothing,
    /// The Box could not tell, or the work is not something it can see.
    Unknown,
}

/// Whether `record` is a wait quiet long enough to ask the Box about it.
///
/// A subagent's tool calls report busy, so a wait this quiet has no subagent between tools.
#[must_use]
pub fn wait_overdue(record: &HookRecord, now: Ts) -> bool {
    matches!(record.kind, HookKind::Waiting { .. }) && stalled_for(record.at, now).is_some()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::stall::STALL_AFTER;
    use super::*;

    fn report(kind: HookKind) -> HookRecord {
        HookRecord {
            kind,
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: Ts::from_millis(1_000_000),
            turn: None,
        }
    }

    fn waiting() -> HookRecord {
        report(HookKind::Waiting {
            on: vec!["gh run watch".to_owned()],
        })
    }

    #[test]
    fn a_wait_is_overdue_only_once_quiet_for_the_stall_bound() {
        let rec = waiting();
        let just_under = rec
            .at
            .saturating_add(STALL_AFTER.checked_sub(Duration::from_millis(1)).unwrap());
        assert!(!wait_overdue(&rec, just_under));
        assert!(wait_overdue(&rec, rec.at.saturating_add(STALL_AFTER)));
    }

    #[test]
    fn only_a_wait_is_ever_overdue() {
        let rec = report(HookKind::Idle);
        assert!(!wait_overdue(&rec, rec.at.saturating_add(STALL_AFTER * 10)));
        let rec = report(HookKind::Busy);
        assert!(!wait_overdue(&rec, rec.at.saturating_add(STALL_AFTER * 10)));
    }

    #[test]
    fn a_clock_behind_the_record_is_not_overdue() {
        assert!(!wait_overdue(&waiting(), Ts::from_millis(500_000)));
    }
}
