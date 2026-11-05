//! Tunnel rates: a bucket of `burst` tunnels that refills at `per_sec` a second.
//!
//! Each token whose Box the ledger confirmed gets a bucket of its own. Every other token draws on
//! one shared lookup bucket before its ledger lookup, so random tokens neither grow a map nor
//! flood the controller.

use std::time::Instant;

/// How fast a bucket refills and how much it holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rate {
    /// Tunnels a second.
    pub(crate) per_sec: f64,
    /// Bucket size.
    pub(crate) burst: f64,
}

/// One bucket.
pub(super) struct Bucket {
    /// Tunnels it may still open.
    left: f64,
    /// When `left` was last topped up.
    at: Instant,
}

impl Bucket {
    /// A bucket holding `left` tunnels at `now`.
    pub(super) const fn new(left: f64, now: Instant) -> Self {
        Self { left, at: now }
    }

    /// Takes one tunnel at `now`; false when the bucket is empty.
    pub(super) fn take(&mut self, rate: Rate, now: Instant) -> bool {
        let gained = now.saturating_duration_since(self.at).as_secs_f64() * rate.per_sec;
        self.left = (self.left + gained).min(rate.burst);
        self.at = now;
        if self.left < 1.0 {
            return false;
        }
        self.left -= 1.0;
        true
    }
}
