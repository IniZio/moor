//! Event stream and settle pass for the controller, behind the `Moor` facade.

use moor_core::controller::ServerFrame;
use moor_core::view::TaskView;
use moor_core::ShowReq;

use super::Moor;
use crate::ops;
use crate::store::reads::NamedEvent;

impl Moor {
    /// Rows of every open Task, as `show` lists them.
    pub(crate) fn open_tasks(&self) -> Result<Vec<TaskView>, String> {
        let req = ShowReq {
            task: None,
            events: None,
            agent: false,
            reports: false,
            pane: false,
        };
        ops::show::run(&self.cx, &req)
            .map(|v| v.tasks.unwrap_or_default())
            .map_err(|e| e.error.to_string())
    }

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

    /// Settle every open Task once, as `start` does; returns failure and warning messages to log.
    ///
    /// Autosleep runs with it, so an agent idle past its limit sleeps without a client. New or
    /// changed proof files are recorded and copied too, up to `proof_cap` bytes each.
    pub(crate) fn settle_open(&self, proof_cap: u64) -> Vec<String> {
        let settled = self
            .cx
            .store()
            .and_then(|store| ops::settle_all(&self.cx, store));
        let mut out = Vec::new();
        if let Err(e) = settled {
            out.push(e.error.to_string());
        }
        if let Ok(store) = self.cx.store() {
            ops::proof::sync_proofs(&self.cx, store, proof_cap);
        }
        out.extend(self.cx.take_warnings().into_iter().map(|w| w.message));
        out
    }
}

#[cfg(test)]
mod tests;
