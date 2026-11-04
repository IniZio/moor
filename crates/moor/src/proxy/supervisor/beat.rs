//! The lease heartbeat, which rides out a controller that is briefly unreachable.

use std::cell::Cell;
use std::time::{Duration, Instant};

use super::run::{say, Running, Stop};
use super::{LEASE, TICK};
use crate::proxy::ledger::LedgerError;

/// How long the supervisor keeps serving with no heartbeat the controller answered. It stops
/// before the lease runs out, so a supervisor a restarted controller starts finds the port free.
pub(super) const CUT_OFF: Duration = LEASE.saturating_sub(TICK.saturating_mul(2));

/// When the controller last answered a heartbeat, and whether the latest one reached it.
pub(super) struct Contact {
    /// The last answered heartbeat.
    at: Cell<Instant>,
    /// Whether the latest heartbeat reached the controller.
    reached: Cell<bool>,
}

impl Contact {
    /// Contact made at `at`.
    pub(super) const fn new(at: Instant) -> Self {
        Self {
            at: Cell::new(at),
            reached: Cell::new(true),
        }
    }

    /// Whether the latest heartbeat reached the controller.
    pub(super) const fn reached(&self) -> bool {
        self.reached.get()
    }
}

impl Running<'_> {
    /// Extends the lease; `None` means it is still held, or the controller has been unreachable
    /// for less than [`CUT_OFF`].
    pub(super) fn beat(&self) -> Option<Stop> {
        let now = self.deps.clock.instant();
        let outcome = self.deps.ledger.heartbeat(&self.owner);
        self.contact
            .reached
            .set(!matches!(outcome, Err(LedgerError::Unavailable(_))));
        match outcome {
            Ok(true) => {
                self.contact.at.set(now);
                None
            }
            Ok(false) | Err(LedgerError::Refused(_)) => Some(Stop::LostLease),
            Err(LedgerError::Unavailable(err)) => {
                let silent = now.saturating_duration_since(self.contact.at.get());
                if silent < CUT_OFF {
                    return None;
                }
                say(
                    &self.deps.stderr,
                    format_args!(
                        "the controller has not answered for {}s: {err}",
                        silent.as_secs()
                    ),
                );
                Some(Stop::Unreachable)
            }
            Err(err) => {
                say(&self.deps.stderr, format_args!("heartbeat failed: {err}"));
                Some(Stop::Failed)
            }
        }
    }
}
