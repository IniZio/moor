//! Event stream and settle pass for the controller, behind the `Moor` facade.

use std::time::Duration;

use moor_core::controller::ServerFrame;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::event::close_reason::CLOSE_REASON_SETUP_FAILED_EXPIRED;
use moor_core::{BoxId, SessionId};

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
    #[cfg(test)]
    pub(crate) fn settle_open(&self, draining: &dyn Fn() -> bool) -> Vec<String> {
        self.settle_open_closing(draining, None)
    }

    /// `settle_open`, which also closes each Box that has stayed `setup_failed` for
    /// `close_after`; `None` closes none.
    pub(crate) fn settle_open_closing(
        &self,
        draining: &dyn Fn() -> bool,
        close_after: Option<Duration>,
    ) -> Vec<String> {
        let mut out = Vec::new();
        match self.cx.store() {
            Ok(store) => {
                if let Err(e) = ops::settle_all(&self.cx, store, draining) {
                    out.push(e.error.to_string());
                }
                ops::common::artifacts::collect_due(&self.cx, store, draining);
                ops::common::artifacts::collect_working(&self.cx, store, draining);
                if let Some(after) = close_after {
                    out.extend(self.close_expired_setup_failed(store, after, draining));
                }
            }
            Err(e) => out.push(e.error.to_string()),
        }
        out.extend(self.cx.take_warnings().into_iter().map(|w| w.message));
        out
    }

    /// Closes each open Box whose setup failed at least `after` ago, once per call.
    ///
    /// Setup never handed such a Box to anyone, so the close is forced. A Box with no failure
    /// time is left alone, and one that will not close is reported, not retried until the next
    /// pass.
    fn close_expired_setup_failed(
        &self,
        store: &crate::store::Store,
        after: Duration,
        draining: &dyn Fn() -> bool,
    ) -> Vec<String> {
        let now = self.cx.clock().now().as_millis();
        let after_ms = i64::try_from(after.as_millis()).unwrap_or(i64::MAX);
        let rows = match store.open_boxes() {
            Ok(rows) => rows,
            Err(e) => return vec![e.to_string()],
        };
        let mut out = Vec::new();
        for row in rows {
            let expired = row.lifecycle == BoxLifecycle::SetupFailed
                && row
                    .setup_failed_at
                    .is_some_and(|at| at.as_millis().saturating_add(after_ms) <= now);
            if !expired {
                continue;
            }
            if draining() {
                break;
            }
            let view = self.cx.for_box(row.user, || {
                ops::close_failed::close_setup_failed(
                    &self.cx,
                    std::slice::from_ref(&row),
                    CLOSE_REASON_SETUP_FAILED_EXPIRED,
                    true,
                )
            });
            out.extend(view.failed.into_iter().map(|f| {
                format!(
                    "could not close setup_failed Box {}: {}",
                    f.r#box.as_str(),
                    f.error
                )
            }));
        }
        out
    }

    /// Fails each start an earlier process left unfinished, claiming those whose setup can be
    /// followed again; returns what to log and the claimed Boxes.
    pub(crate) fn fail_interrupted_starts(&self) -> (Vec<String>, Vec<BoxId>) {
        let mut out = Vec::new();
        let mut claimed = Vec::new();
        let failed = self.cx.store().and_then(|store| {
            claimed = ops::common::provision::interrupted::fail_interrupted(&self.cx, store)?;
            ops::common::node::backfill(&self.cx, store)
        });
        if let Err(e) = failed {
            out.push(e.error.to_string());
        }
        out.extend(self.cx.take_warnings().into_iter().map(|w| w.message));
        (out, claimed)
    }

    /// Follows again the setup of each Box in `claimed` until it ends; returns what to log.
    pub(crate) fn resume_setups(&self, claimed: &[BoxId]) -> Vec<String> {
        let mut out = Vec::new();
        for &id in claimed {
            let resumed = self.cx.store().and_then(|store| {
                let user = store.box_row(id)?.and_then(|r| r.user);
                self.cx
                    .for_box(user, || ops::common::provision::resume_setup(&self.cx, store, id))
            });
            if let Err(e) = resumed {
                out.push(format!("Box {}: {}", id.get(), e.error));
            }
            out.extend(self.cx.take_warnings().into_iter().map(|w| w.message));
        }
        out
    }
}

#[cfg(test)]
mod tests;
