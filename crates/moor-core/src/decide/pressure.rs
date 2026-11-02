//! Relieving a node under memory pressure: which one Box to suspend on a report, if any.

use serde::{Deserialize, Serialize};

use crate::decide::preempt::{longest_idle, Resident};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::name::{BoxId, NodeName};

/// PSI `some avg10`, in hundredths, that counts as pressure when it holds.
pub const SUSTAINED_PSI_CENTI: u32 = 1000;
/// How many reports in a row the PSI must hold to be sustained.
pub const SUSTAINED_REPORTS: usize = 3;
/// PSI `full avg10`, in hundredths, at and above which the host is critical.
pub const CRITICAL_FULL_CENTI: u32 = 500;
/// `MemAvailable` below which the host is critical, in MiB.
pub const CRITICAL_AVAILABLE_MIB: u64 = 1024;
/// How long a node suspends nothing more after a suspension, in seconds.
pub const COOLDOWN_SECS: i64 = 30;

/// One memory report of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    /// When it arrived, unix seconds.
    pub at: i64,
    /// `MemAvailable`, in MiB.
    pub available_mib: u64,
    /// PSI `some avg10`, in hundredths.
    pub psi_some_centi: Option<u32>,
    /// PSI `full avg10`, in hundredths.
    pub psi_full_centi: Option<u32>,
}

/// A Box on the node with when it started, unix seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The Box as the idle choice sees it.
    pub resident: Resident,
    /// When it started.
    pub started_at: i64,
}

/// The one Box to suspend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suspend {
    /// The Box.
    pub id: BoxId,
    /// Whether it was working, so it resumes first and is told about the suspension.
    pub last_resort: bool,
    /// Why, for its events.
    pub reason: String,
}

/// Whether `readings`, oldest first, show sustained pressure: PSI held for the last
/// [`SUSTAINED_REPORTS`] reports, or the latest leaves less than `reserve_mib` available.
#[must_use]
pub fn sustained(readings: &[Reading], reserve_mib: u64) -> bool {
    let Some(last) = readings.last() else {
        return false;
    };
    let held = readings
        .len()
        .checked_sub(SUSTAINED_REPORTS)
        .and_then(|start| readings.get(start..))
        .is_some_and(|tail| {
            tail.iter()
                .all(|r| r.psi_some_centi.is_some_and(|p| p >= SUSTAINED_PSI_CENTI))
        });
    held || last.available_mib < reserve_mib
}

/// Whether `last` shows the host about to thrash or run out.
#[must_use]
pub fn critical(last: &Reading) -> bool {
    last.psi_full_centi
        .is_some_and(|p| p >= CRITICAL_FULL_CENTI)
        || last.available_mib < CRITICAL_AVAILABLE_MIB
}

/// The Box to suspend on `node` after its latest report, if any.
///
/// The longest-idle one under pressure, else, when the host is critical, the most recently
/// started working one. `suspended_at` is the node's last suspension and `now` the time, both unix seconds.
#[must_use]
pub fn relieve(
    node: &NodeName,
    readings: &[Reading],
    reserve_mib: u64,
    boxes: &[Candidate],
    suspended_at: Option<i64>,
    now: i64,
) -> Option<Suspend> {
    let last = readings.last()?;
    if suspended_at.is_some_and(|at| now.saturating_sub(at) < COOLDOWN_SECS) {
        return None;
    }
    let critical = critical(last);
    if !sustained(readings, reserve_mib) && !critical {
        return None;
    }
    let residents: Vec<Resident> = boxes.iter().map(|c| c.resident.clone()).collect();
    if let Some(id) = longest_idle(&residents) {
        return Some(Suspend {
            id,
            last_resort: false,
            reason: format!("memory pressure on {node}"),
        });
    }
    if !critical {
        return None;
    }
    boxes
        .iter()
        .filter(|c| working(&c.resident))
        .max_by_key(|c| (c.started_at, c.resident.id))
        .map(|c| Suspend {
            id: c.resident.id,
            last_resort: true,
            reason: format!("last-resort suspend: memory critical on {node}"),
        })
}

/// Whether `r` is up with its agent mid-turn, not waiting on a person.
#[must_use]
pub fn working(r: &Resident) -> bool {
    r.lifecycle == BoxLifecycle::Ready && r.session == SessionState::Working && !r.needs_input
}

#[cfg(test)]
mod tests;
