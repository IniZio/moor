//! Ending a running supervisor while keeping its row, so another starts on the same ports.

use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::BoxId;

use super::{DetachedSupervisor, Sent};
use crate::store::{ProxyRow, Store};

impl DetachedSupervisor {
    /// Ends the Box's running supervisor and waits for it to give up its lease; the row and
    /// its ports stay.
    pub(crate) fn end(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        let failed = |e: &dyn std::fmt::Display| {
            ProxyHostError::Failed(format!("the proxy state could not be read: {e}"))
        };
        let store = self.store().map_err(|e| failed(&e))?;
        let Some(row) = store.proxy(box_id).map_err(|e| failed(&e))? else {
            return Ok(());
        };
        if self.terminate(store, &row) {
            Ok(())
        } else {
            Err(ProxyHostError::Incomplete(
                "the proxy supervisor did not exit within 3s".to_owned(),
            ))
        }
    }

    /// Sends SIGTERM to the row's supervisor and waits for it to give up its lease; false when
    /// it could not be signalled or outlasts the wait.
    pub(super) fn terminate(&self, store: &Store, row: &ProxyRow) -> bool {
        // Pids 0 and 1 would address every process or init's group. After a reboot the pid may
        // belong to an unrelated group even though the heartbeat looks fresh.
        let pid = row
            .pid
            .filter(|p| *p > 1 && self.live(row) && self.same_boot(row));
        pid.is_none_or(|p| match self.signal_group(p) {
            Sent::Delivered => self.wait_gone(store, row.box_id),
            Sent::Gone => true,
            Sent::Failed => false,
        })
    }
}
