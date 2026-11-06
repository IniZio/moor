//! A node's live memory and disk, and whether another Box fits.

use crate::name::NodeName;

/// A node's latest live memory report and the Boxes booting on it, in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Memory {
    /// The host's total memory.
    pub total_mib: u64,
    /// What stays with the host; `None` means [`default_reserve_mib`].
    pub reserve_mib: Option<u64>,
    /// The host's `MemAvailable` when it last reported.
    pub available_mib: u64,
    /// The host's memory pressure, PSI `some avg10` in hundredths; `None` when it has no PSI.
    pub psi_centi: Option<u32>,
    /// When it last reported, unix seconds.
    pub reported_at: i64,
    /// Boxes Starting on it.
    pub starting: usize,
    /// Free MiB on the filesystem holding the host's Box data; `None` from an agent that does
    /// not report disk, which is then admitted without a disk check.
    pub disk_free_mib: Option<u64>,
    /// The size of that filesystem, in MiB.
    pub disk_total_mib: Option<u64>,
}

/// What one more Box must find free to boot.
pub const BOOT_FLOOR_MIB: u64 = 2048;
/// What a Box resuming on a node with nothing running leaves available, in place of the reserve,
/// so that a host whose own work ate into the reserve does not hold its only Box asleep.
pub const ALONE_MARGIN_MIB: u64 = 1024;
/// How old a memory report may be before the node starts nothing more.
pub const REPORT_FRESH_SECS: i64 = 60;
/// PSI `some avg10`, in hundredths, at and above which the node starts nothing more.
pub const PSI_LIMIT_CENTI: u32 = 1000;

/// A node keeps this share of its disk free before it takes another Box.
pub const DISK_FLOOR_PERCENT: u64 = 10;
/// A node keeps at least this much disk free, in MiB, before it takes another Box.
pub const DISK_FLOOR_MIB: u64 = 20 * 1024;

/// What a node of `total_mib` keeps free: [`DISK_FLOOR_PERCENT`] of it, at least [`DISK_FLOOR_MIB`].
#[must_use]
pub const fn disk_floor_mib(total_mib: u64) -> u64 {
    let share = total_mib.saturating_mul(DISK_FLOOR_PERCENT) / 100;
    if share > DISK_FLOOR_MIB {
        share
    } else {
        DISK_FLOOR_MIB
    }
}

/// The host's share when none is configured: 4 GiB plus a tenth of `total_mib`.
#[must_use]
pub const fn default_reserve_mib(total_mib: u64) -> u64 {
    4096 + total_mib / 10
}

/// Why a node has no room for one more Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortfall {
    /// Its last memory report is too old.
    Stale,
    /// Its host is under memory pressure.
    Pressure,
    /// Too little disk is free on the host.
    Disk {
        /// What is free.
        free_mib: u64,
        /// What must be free.
        needs_mib: u64,
    },
    /// Too little is free.
    Low {
        /// What is free for another Box.
        free_mib: u64,
        /// What the Box needs free.
        needs_mib: u64,
    },
    /// A Box came up after its last report, which does not count that Box yet.
    Unreported,
}

impl Shortfall {
    /// Why a Box waits for `node`, for the user.
    #[must_use]
    pub fn reason(&self, node: &NodeName) -> String {
        match self {
            Self::Stale => format!("no recent memory report from {node}"),
            Self::Pressure => format!("memory pressure on {node}"),
            Self::Disk {
                free_mib,
                needs_mib,
            } => format!(
                "low disk on {node}: {} free, needs {}",
                gib(*free_mib),
                gib(*needs_mib)
            ),
            Self::Low {
                free_mib,
                needs_mib,
            } => format!(
                "waiting for capacity on {node}: {} free, needs {}",
                gib(*free_mib),
                gib(*needs_mib)
            ),
            Self::Unreported => format!("waiting for the next memory report from {node}"),
        }
    }
}

impl Memory {
    /// What stays with the host.
    #[must_use]
    pub const fn reserve(&self) -> u64 {
        match self.reserve_mib {
            Some(r) => r,
            None => default_reserve_mib(self.total_mib),
        }
    }

    /// What is free for another Box: available less the reserve and a boot floor per Box
    /// Starting, since a booting guest has not shown up in the report yet.
    #[must_use]
    pub const fn free_mib(&self) -> u64 {
        let booting = BOOT_FLOOR_MIB.saturating_mul(self.starting as u64);
        self.available_mib
            .saturating_sub(self.reserve())
            .saturating_sub(booting)
    }

    /// The disk short of its floor, the larger of [`DISK_FLOOR_PERCENT`] of the filesystem and
    /// [`DISK_FLOOR_MIB`]; `None` when it is above it or the host does not report disk.
    fn disk_shortfall(&self) -> Option<Shortfall> {
        let (free_mib, total_mib) = (self.disk_free_mib?, self.disk_total_mib?);
        let needs_mib = disk_floor_mib(total_mib);
        (free_mib < needs_mib).then_some(Shortfall::Disk {
            free_mib,
            needs_mib,
        })
    }

    /// Why the host takes no Box at `now` whatever its size; `None` when it may.
    fn unfit(&self, now: i64) -> Option<Shortfall> {
        if now.saturating_sub(self.reported_at) > REPORT_FRESH_SECS {
            return Some(Shortfall::Stale);
        }
        if self.psi_centi.is_some_and(|p| p >= PSI_LIMIT_CENTI) {
            return Some(Shortfall::Pressure);
        }
        self.disk_shortfall()
    }

    /// Why one more Box does not fit at `now`, unix seconds; `None` when it does.
    #[must_use]
    pub fn shortfall(&self, now: i64) -> Option<Shortfall> {
        self.unfit(now).or_else(|| {
            let free_mib = self.free_mib();
            (free_mib < BOOT_FLOOR_MIB).then_some(Shortfall::Low {
                free_mib,
                needs_mib: BOOT_FLOOR_MIB,
            })
        })
    }

    /// Why an asleep Box does not resume at `now`, unix seconds; `None` when it may.
    ///
    /// It needs `ask.needed_mib` free above the reserve, or, with nothing running,
    /// [`ALONE_MARGIN_MIB`] more than that available. A Box of unknown size asks what a new start
    /// does, reserve and all.
    #[must_use]
    pub fn resume_shortfall(&self, now: i64, ask: ResumeAsk) -> Option<Shortfall> {
        if let Some(short) = self.unfit(now) {
            return Some(short);
        }
        if ask.unreported {
            return Some(Shortfall::Unreported);
        }
        let free_mib = self.free_mib();
        let needs_mib = ask.needed_mib.unwrap_or(BOOT_FLOOR_MIB);
        let alone = ask.running == 0
            && ask.needed_mib.is_some()
            && self.available_mib >= needs_mib.saturating_add(ALONE_MARGIN_MIB);
        (free_mib < needs_mib && !alone).then_some(Shortfall::Low {
            free_mib,
            needs_mib,
        })
    }
}

/// What a resume asks of its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumeAsk {
    /// MiB it needs, from [`needed_to_resume`]; none when nothing says.
    pub needed_mib: Option<u64>,
    /// Boxes running or starting on the node.
    pub running: usize,
    /// Whether a Box came up after the node's last report.
    pub unreported: bool,
}

/// The MiB a Box needs to resume: what its provider reports, else its guest's whole memory.
///
/// A guest's ceiling can far exceed its real use, so a provider's number keeps such a Box from
/// waiting for room it would never fill.
#[must_use]
pub fn needed_to_resume(reported_mib: Option<u64>, guest_mib: Option<u64>) -> Option<u64> {
    reported_mib.filter(|m| *m > 0).or(guest_mib)
}

/// `mib` as GiB for a message: whole when it divides evenly, else one decimal.
#[must_use]
pub fn gib(mib: u64) -> String {
    if mib.is_multiple_of(1024) {
        format!("{} GiB", mib / 1024)
    } else {
        #[expect(clippy::cast_precision_loss, reason = "a message rounds it anyway")]
        let g = mib as f64 / 1024.0;
        format!("{g:.1} GiB")
    }
}

#[cfg(test)]
mod tests;
