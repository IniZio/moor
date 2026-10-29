//! Event stream and settle pass for the controller, behind the `Moor` facade.

use moor_core::controller::ServerFrame;
use moor_core::SessionId;

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

    /// The parent of Session `id`; none when it has no parent or does not exist.
    ///
    /// # Errors
    /// The store failure, as text.
    pub(crate) fn session_parent(&self, id: SessionId) -> Result<Option<SessionId>, String> {
        let store = self.cx.store().map_err(|e| e.error.to_string())?;
        let row = store.session_row(id.get()).map_err(|e| e.to_string())?;
        Ok(row.and_then(|r| r.parent).map(SessionId::new))
    }

    /// A pass that refreshes users' OAuth MCP access tokens near expiry and returns lines to
    /// log. The pass keeps each server's retry backoff from one run to the next.
    pub(crate) fn oauth_refresher(
        env: &crate::env::Env,
        vault: std::sync::Arc<crate::vault::Vault>,
    ) -> impl Fn(&dyn Fn() -> bool) -> Vec<String> + Send + 'static {
        let env = env.clone();
        let retries = std::sync::Mutex::new(ops::users::mcp::oauth::refresh::Retries::default());
        move |_| match Self::from_env(&env) {
            Ok(moor) => {
                let mut retries = retries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ops::users::mcp::oauth::refresh::refresh_due(&moor.cx, &vault, &mut retries)
            }
            Err(e) => vec![e.error.to_string()],
        }
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
                ops::common::artifacts::collect_working(&self.cx, store, draining);
            }
            Err(e) => out.push(e.error.to_string()),
        }
        out.extend(self.cx.take_warnings().into_iter().map(|w| w.message));
        out
    }

    /// Fails each start an earlier process left unfinished; returns what to log.
    pub(crate) fn fail_interrupted_starts(&self) -> Vec<String> {
        let mut out = Vec::new();
        let failed = self.cx.store().and_then(|store| {
            ops::common::provision::interrupted::fail_interrupted(&self.cx, store)
        });
        if let Err(e) = failed {
            out.push(e.error.to_string());
        }
        out.extend(self.cx.take_warnings().into_iter().map(|w| w.message));
        out
    }
}

#[cfg(test)]
mod tests;
