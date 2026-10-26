//! Progress on a running call. A client ends an HTTP call that sends it nothing for a while, so a
//! long `sync` or `exec` reports how long it has run at each beat. Each call's progress only goes
//! up, so the hold wait and the beat share one counter.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::{Peer, RequestContext};
use rmcp::RoleServer;

/// Where one call's progress goes; a call without a progress token gets none.
#[derive(Clone)]
pub(crate) struct Progress {
    /// The caller.
    peer: Peer<RoleServer>,
    /// The token the caller asked progress on.
    token: Option<ProgressToken>,
    /// The last progress value sent.
    sent: Arc<Mutex<f64>>,
}

impl Progress {
    /// The progress channel of the call `context` serves.
    pub(crate) fn of(context: &RequestContext<RoleServer>) -> Self {
        Self {
            peer: context.peer.clone(),
            token: context.meta.get_progress_token(),
            sent: Arc::default(),
        }
    }

    /// Sends `message` as the next step.
    pub(crate) async fn note(&self, message: String) {
        let Some(token) = self.token.clone() else {
            return;
        };
        let step = {
            let mut sent = self.sent.lock().unwrap_or_else(PoisonError::into_inner);
            *sent += 1.0;
            *sent
        };
        let note = ProgressNotificationParam::new(token, step).with_message(message);
        let _ = self.peer.notify_progress(note).await;
    }

    /// Notes `what` and its run time every `every` until dropped.
    pub(crate) async fn beat(&self, what: &str, every: Duration) {
        let started = tokio::time::Instant::now();
        let mut tick = tokio::time::interval_at(started + every, every);
        loop {
            tick.tick().await;
            let secs = started.elapsed().as_secs();
            self.note(format!("{what}: running for {secs} s")).await;
        }
    }
}
