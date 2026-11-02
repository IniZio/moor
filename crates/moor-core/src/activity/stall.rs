//! How long an agent's hook may stay quiet before its turn counts as stalled.

use std::time::Duration;

use crate::seam::clock::Ts;

/// How long a busy hook may go without a write before the turn counts as stalled.
pub const STALL_AFTER: Duration = Duration::from_secs(600);

/// Whole seconds the hook has been quiet, or `None` while under [`STALL_AFTER`].
#[must_use]
pub fn stalled_for(record_at: Ts, now: Ts) -> Option<u64> {
    let quiet_ms = u64::try_from(now.as_millis().checked_sub(record_at.as_millis())?).ok()?;
    let quiet = Duration::from_millis(quiet_ms);
    (quiet >= STALL_AFTER).then_some(quiet.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_the_threshold_is_not_a_stall() {
        let at = Ts::from_millis(1_000_000);
        assert_eq!(
            stalled_for(at, at.saturating_add(Duration::from_millis(599_999))),
            None
        );
    }

    #[test]
    fn exactly_the_threshold_is_a_stall() {
        let at = Ts::from_millis(1_000_000);
        assert_eq!(stalled_for(at, at.saturating_add(STALL_AFTER)), Some(600));
    }

    #[test]
    fn a_clock_behind_the_record_is_not_a_stall() {
        let at = Ts::from_millis(1_000_000);
        assert_eq!(stalled_for(at, Ts::from_millis(500_000)), None);
    }
}
