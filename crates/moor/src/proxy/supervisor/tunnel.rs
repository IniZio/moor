//! Restarting a dead reverse tunnel with a fixed backoff.

use std::time::Duration;

use super::bind;
use super::run::{say, Running, Stop};
use crate::store::Fenced;

/// Waits before each successive restart attempt; running out of them gives up.
const BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
];

/// Why a restart is recorded.
const REASON: &str = "tunnel exited";

impl Running<'_> {
    /// Replaces the dead tunnel; `None` means a new one is up and recorded.
    ///
    /// The lease is renewed before each backoff wait, during each restart attempt and after it,
    /// so neither the wait nor a slow attempt lets it lapse.
    pub(super) fn revive(&self) -> Option<Stop> {
        let sink = &self.deps.stderr;
        say(
            sink,
            format_args!("the tunnel into the box exited; restarting"),
        );
        let mut last = String::new();
        for wait in BACKOFF {
            if let stop @ Some(_) = self.beat() {
                return stop;
            }
            self.deps.clock.sleep(wait);
            match bind::open(
                self.deps,
                (self.box_id, &self.owner),
                &self.handle,
                (self.box_port, self.live.socket()),
                false,
            ) {
                Ok(tunnel) => {
                    *self.tunnel.borrow_mut() = tunnel;
                    if let stop @ Some(_) = self.beat() {
                        return stop;
                    }
                    return self.record();
                }
                Err(err) => {
                    last = err.to_string();
                    say(sink, format_args!("cannot restart the tunnel: {last}"));
                    if let stop @ Some(_) = self.beat() {
                        return stop;
                    }
                }
            }
        }
        say(
            sink,
            format_args!(
                "the tunnel into the box did not restart after {} attempts ({last}); stopping",
                BACKOFF.len()
            ),
        );
        Some(Stop::TunnelDead)
    }

    /// Records the restart; `None` unless the lease was lost or the store failed.
    fn record(&self) -> Option<Stop> {
        match self
            .deps
            .store
            .tunnel_restarted(self.box_id, &self.owner, self.box_port, REASON)
        {
            Ok(Fenced::Applied) => None,
            Ok(Fenced::LostLease) => Some(Stop::LostLease),
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot record the restart: {err}"),
                );
                Some(Stop::Failed)
            }
        }
    }
}

#[cfg(test)]
mod tests;
