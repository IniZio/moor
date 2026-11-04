//! Restarting a dead reverse tunnel: a quick burst of attempts, then one every `RETRY` until it is back.

use std::cell::Cell;
use std::time::Duration;

use super::bind;
use super::run::{say, Running, Stop};
use crate::store::Fenced;

/// Waits before each attempt of the quick burst that follows a tunnel exit.
const BURST: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
];

/// Pause between attempts once the burst has failed; the tunnel rides out a link outage of any length.
const RETRY: Duration = Duration::from_secs(30);

/// A tunnel that stayed down through the burst.
#[derive(Debug, Clone, Copy)]
pub(super) struct Outage {
    /// When the tunnel was found dead, in clock milliseconds.
    since: i64,
    /// When the next attempt is due, in clock milliseconds.
    next: i64,
}

/// How one restart attempt went.
enum Attempt {
    /// A new tunnel is up.
    Up,
    /// The open failed with this error; the lease is still held.
    Failed(String),
    /// The lease was lost or the store failed.
    Stop(Stop),
}

/// The current outage, if any; `None` while the tunnel is up or the burst is still to run.
pub(super) type OutageSlot = Cell<Option<Outage>>;

impl Running<'_> {
    /// Works on a dead tunnel; `None` means keep running, with the tunnel back or still down.
    ///
    /// The first call after an exit runs the quick burst, renewing the lease around every wait and
    /// attempt. Later calls try once whenever `RETRY` has passed, so the ticks between them keep
    /// heartbeating and noticing a closed Box or a park request.
    pub(super) fn revive(&self) -> Option<Stop> {
        let now = self.deps.clock.now().as_millis();
        match self.outage.get() {
            None => self.burst(now),
            Some(outage) if now < outage.next => None,
            Some(outage) => self.retry(outage, now),
        }
    }

    /// Runs the quick burst; when it fails, starts the outage.
    fn burst(&self, since: i64) -> Option<Stop> {
        let sink = &self.deps.stderr;
        say(
            sink,
            format_args!("the tunnel into the box exited; restarting"),
        );
        let mut last = String::new();
        for wait in BURST {
            if let stop @ Some(_) = self.beat() {
                return stop;
            }
            self.deps.clock.sleep(wait);
            match self.attempt() {
                Attempt::Up => return self.back("tunnel exited"),
                Attempt::Stop(stop) => return Some(stop),
                Attempt::Failed(err) => last = err,
            }
        }
        say(
            sink,
            format_args!(
                "the tunnel into the box did not restart after {} attempts ({last}); retrying every {} s",
                BURST.len(),
                RETRY.as_secs()
            ),
        );
        self.outage.set(Some(Outage {
            since,
            next: self.due(),
        }));
        None
    }

    /// One attempt during an outage.
    fn retry(&self, outage: Outage, now: i64) -> Option<Stop> {
        match self.attempt() {
            Attempt::Up => {
                let down = (now - outage.since) / 1000;
                self.back(&format!("tunnel exited; back after {down} s"))
            }
            Attempt::Stop(stop) => Some(stop),
            Attempt::Failed(_) => {
                self.outage.set(Some(Outage {
                    next: self.due(),
                    ..outage
                }));
                None
            }
        }
    }

    /// When the next outage attempt is due.
    fn due(&self) -> i64 {
        let retry = i64::try_from(RETRY.as_millis()).unwrap_or(i64::MAX);
        self.deps.clock.now().as_millis().saturating_add(retry)
    }

    /// Opens a new tunnel in place of the dead one, reporting a failure; the lease is renewed after.
    fn attempt(&self) -> Attempt {
        let opened = bind::open(
            self.deps,
            &self.owner,
            &self.handle,
            (self.box_port, self.live.socket()),
            false,
        );
        let failed = match opened {
            Ok(tunnel) => {
                *self.tunnel.borrow_mut() = tunnel;
                None
            }
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot restart the tunnel: {err}"),
                );
                Some(err.to_string())
            }
        };
        match (self.beat(), failed) {
            (Some(stop), _) => Attempt::Stop(stop),
            (None, None) => Attempt::Up,
            (None, Some(err)) => Attempt::Failed(err),
        }
    }

    /// Ends the outage and records the restart.
    fn back(&self, reason: &str) -> Option<Stop> {
        self.outage.set(None);
        self.record(reason)
    }

    /// Records the restart; `None` unless the lease was lost or the store failed.
    fn record(&self, reason: &str) -> Option<Stop> {
        match self
            .deps
            .ledger
            .tunnel_restarted(&self.owner, self.box_port, reason)
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
