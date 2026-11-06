//! Relieving a node under memory pressure: which one Box to suspend on a report, if any.

use std::cmp::Reverse;

use serde::{Deserialize, Serialize};

use crate::decide::preempt::{suspendable, Resident};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::name::{BoxId, NodeName};
use crate::seam::clock::Ts;

/// PSI `some avg10`, in hundredths, that counts as pressure when it holds.
pub const SUSTAINED_PSI_CENTI: u32 = 1000;
/// How many reports in a row the PSI must hold to be sustained.
pub const SUSTAINED_REPORTS: usize = 3;
/// PSI `full avg10`, in hundredths, at and above which the host is critical.
pub const CRITICAL_FULL_CENTI: u32 = 500;
/// `MemAvailable` below which the host is critical, in MiB.
pub const CRITICAL_AVAILABLE_MIB: u64 = 1024;
/// Boxes that must run on a node before a working one may be suspended: the only one never is,
/// since the queue it would join has nothing ahead of it but itself.
pub const LAST_RESORT_RUNNING: usize = 2;
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
    /// Whether the node's disk counted as low once this report was judged.
    #[serde(default)]
    pub disk_low: bool,
}

/// A Box on the node with when it started, unix seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The Box as the idle choice sees it.
    pub resident: Resident,
    /// When it started.
    pub started_at: i64,
    /// Whether a run is in flight in it, which suspending it would drop.
    pub in_flight: bool,
}

/// What a node's waiting Boxes ask of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Demand {
    /// Whether the node is short of memory for another Box.
    pub short: bool,
    /// The Box that has waited longest for the node.
    pub waiter: Option<String>,
}

/// When the node last suspended a Box and what time it is, both unix seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cooldown {
    /// The node's last suspension.
    pub suspended_at: Option<i64>,
    /// Now.
    pub now: i64,
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

/// The Boxes to try suspending on `node` after its latest report, in order; the first that
/// suspends ends the attempt, and one refused falls through to the next.
///
/// Under pressure, or when a Box waits on a short node, the idle ones, longest idle first. Then,
/// when the host is critical or has less than `reserve_mib` available and at least two Boxes run,
/// the working ones, newest first, those with a run in flight last. Nothing within
/// [`COOLDOWN_SECS`] of the last suspension.
#[must_use]
pub fn relieve(
    node: &NodeName,
    readings: &[Reading],
    reserve_mib: u64,
    boxes: &[Candidate],
    cooldown: Cooldown,
    demand: &Demand,
) -> Vec<Suspend> {
    let Cooldown { suspended_at, now } = cooldown;
    let Some(last) = readings.last() else {
        return Vec::new();
    };
    if suspended_at.is_some_and(|at| now.saturating_sub(at) < COOLDOWN_SECS) {
        return Vec::new();
    }
    let critical = critical(last);
    let pressed = sustained(readings, reserve_mib) || critical;
    let waiter = demand.waiter.as_deref().filter(|_| demand.short);
    if !pressed && waiter.is_none() {
        return Vec::new();
    }
    let reason = match waiter {
        Some(w) if !pressed => format!("made room for {w}"),
        _ => format!("memory pressure on {node}"),
    };
    let mut idle: Vec<&Candidate> = boxes
        .iter()
        .filter(|c| suspendable(&c.resident) && !c.in_flight)
        .collect();
    idle.sort_by_key(|c| (c.resident.idle_since.map(Ts::as_millis), c.resident.id));
    let mut picks: Vec<Suspend> = idle
        .into_iter()
        .map(|c| Suspend {
            id: c.resident.id,
            last_resort: false,
            reason: reason.clone(),
        })
        .collect();
    let running = boxes
        .iter()
        .filter(|c| c.resident.lifecycle == BoxLifecycle::Ready)
        .count();
    if running >= LAST_RESORT_RUNNING && (critical || last.available_mib < reserve_mib) {
        let why = if critical {
            "memory critical"
        } else {
            "memory below the host reserve"
        };
        let mut busy: Vec<&Candidate> = boxes
            .iter()
            .filter(|c| working(&c.resident) || (c.in_flight && suspendable(&c.resident)))
            .collect();
        busy.sort_by_key(|c| (c.in_flight, Reverse((c.started_at, c.resident.id))));
        picks.extend(busy.into_iter().map(|c| Suspend {
            id: c.resident.id,
            last_resort: true,
            reason: format!(
                "last-resort suspend: {why} on {node}{}",
                if c.in_flight {
                    ", with a run in flight"
                } else {
                    ""
                }
            ),
        }));
    }
    picks
}

/// Whether `r` is up with its agent mid-turn, not waiting on a person.
#[must_use]
pub fn working(r: &Resident) -> bool {
    r.lifecycle == BoxLifecycle::Ready && r.session == SessionState::Working && !r.needs_input
}

#[cfg(test)]
mod tests;
