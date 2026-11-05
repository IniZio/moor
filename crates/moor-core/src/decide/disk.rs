//! Whether a node's host disk has run low, judged on each report so a person hears of it once.

use crate::decide::schedule::{disk_floor_mib, REPORT_FRESH_SECS};

/// How far above the admission floor a node must climb to count as recovered, as a share of its
/// disk. Without it a node hovering at the floor would raise and clear the alert on every report.
pub const RECOVERY_MARGIN_PERCENT: u64 = 5;

/// Whether a node's disk is below the floor it keeps for admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// At or above the floor, or never seen below it.
    Ok,
    /// Fell below the floor and has not climbed back past the margin.
    Low,
}

/// What a node's host last reported about its disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// When it arrived, unix seconds.
    pub at: i64,
    /// Free MiB; `None` from an agent that does not report disk.
    pub free_mib: Option<u64>,
    /// The filesystem's size in MiB.
    pub total_mib: Option<u64>,
}

/// A crossing worth telling a person about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The disk fell below the floor.
    WentLow {
        /// Free MiB.
        free_mib: u64,
        /// The filesystem's size in MiB.
        total_mib: u64,
    },
    /// The disk climbed back past the floor and the margin.
    Recovered {
        /// Free MiB.
        free_mib: u64,
        /// The filesystem's size in MiB.
        total_mib: u64,
    },
}

/// The state after `report` at `now`, unix seconds, and the crossing it made, if any.
///
/// A stale report or one without disk figures leaves the state as it was, since neither says the
/// disk changed.
#[must_use]
pub const fn judge(prev: State, report: Report, now: i64) -> (State, Option<Change>) {
    let (Some(free_mib), Some(total_mib)) = (report.free_mib, report.total_mib) else {
        return (prev, None);
    };
    if now.saturating_sub(report.at) > REPORT_FRESH_SECS {
        return (prev, None);
    }
    let floor = disk_floor_mib(total_mib);
    let clear = floor.saturating_add(total_mib.saturating_mul(RECOVERY_MARGIN_PERCENT) / 100);
    match prev {
        State::Ok if free_mib < floor => (
            State::Low,
            Some(Change::WentLow {
                free_mib,
                total_mib,
            }),
        ),
        State::Low if free_mib >= clear => (
            State::Ok,
            Some(Change::Recovered {
                free_mib,
                total_mib,
            }),
        ),
        _ => (prev, None),
    }
}

#[cfg(test)]
mod tests;
