//! Wait-events verdicts.

use std::time::Duration;

use crate::name::BoxName;
use crate::request::WaitMode;

/// Outcome of one wait-events poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitEventsVerdict {
    /// The condition holds.
    Matched,
    /// Keep polling.
    Keep,
    /// The timeout elapsed first.
    TimedOut,
}

/// Decides one poll: a match wins over the timeout, and the timeout fires once
/// `elapsed >= timeout`.
#[must_use]
pub fn wait_events_verdict(
    mode: WaitMode,
    wanted: &[BoxName],
    matched: &[BoxName],
    elapsed: Duration,
    timeout: Duration,
) -> WaitEventsVerdict {
    let hit = |b: &BoxName| matched.contains(b);
    let met = !wanted.is_empty()
        && match mode {
            WaitMode::Any => wanted.iter().any(hit),
            WaitMode::All => wanted.iter().all(hit),
        };
    if met {
        WaitEventsVerdict::Matched
    } else if elapsed >= timeout {
        WaitEventsVerdict::TimedOut
    } else {
        WaitEventsVerdict::Keep
    }
}

#[cfg(test)]
mod tests;
