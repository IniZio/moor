//! The lease heartbeat, which rides out a controller that is unreachable for any length of time.

use std::cell::Cell;

use super::run::{say, Running, Stop};
use crate::proxy::ledger::LedgerError;

/// Whether the latest heartbeat reached the controller.
pub(super) struct Contact {
    /// Whether the latest heartbeat reached the controller.
    reached: Cell<bool>,
}

impl Contact {
    /// Contact just made.
    pub(super) const fn new() -> Self {
        Self {
            reached: Cell::new(true),
        }
    }

    /// Whether the latest heartbeat reached the controller.
    pub(super) const fn reached(&self) -> bool {
        self.reached.get()
    }
}

impl Running<'_> {
    /// Extends the lease; `None` means it is still held, or the controller is unreachable.
    ///
    /// Silence never stops the supervisor: only another owner, a refusal or a store failure do.
    pub(super) fn beat(&self) -> Option<Stop> {
        let outcome = self.deps.ledger.heartbeat(&self.owner);
        self.contact
            .reached
            .set(!matches!(outcome, Err(LedgerError::Unavailable(_))));
        match outcome {
            Ok(true) | Err(LedgerError::Unavailable(_)) => None,
            Ok(false) | Err(LedgerError::Refused(_)) => Some(Stop::LostLease),
            Err(err) => {
                say(&self.deps.stderr, format_args!("heartbeat failed: {err}"));
                Some(Stop::Failed)
            }
        }
    }
}
