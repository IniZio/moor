//! Event stream and settle pass for the controller, behind the `Moor` facade.

use moor_core::controller::ServerFrame;

use super::Moor;
use crate::ops;
use crate::store::reads::NamedEvent;

impl Moor {
    /// Stored events with cursor above `after`, oldest first, at most `limit`, as wire frames.
    pub(crate) fn events_after(
        &self,
        after: i64,
        limit: usize,
    ) -> Result<Vec<ServerFrame>, String> {
        let store = self.cx.store().map_err(|e| e.error.to_string())?;
        let events = store
            .events_after(after, limit)
            .map_err(|e| e.to_string())?;
        Ok(events.into_iter().map(NamedEvent::into_frame).collect())
    }

    /// Settle every open Box once, as `start` does; returns failure and warning messages to log.
    ///
    /// Autosleep runs with it, so an agent idle past its limit sleeps without a client. Each
    /// Box is skipped once `draining` returns true; one already begun finishes first.
    pub(crate) fn settle_open(&self, draining: &dyn Fn() -> bool) -> Vec<String> {
        let mut out = Vec::new();
        match self.cx.store() {
            Ok(store) => {
                if let Err(e) = ops::settle_all(&self.cx, store, draining) {
                    out.push(e.error.to_string());
                }
                ops::common::artifacts::collect_due(&self.cx, store, draining);
            }
            Err(e) => out.push(e.error.to_string()),
        }
        out.extend(self.cx.take_warnings().into_iter().map(|w| w.message));
        out
    }
}

#[cfg(test)]
mod tests;
