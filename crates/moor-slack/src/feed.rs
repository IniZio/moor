//! The feed: folds the controller's events by Box id and keeps each Box's status message in
//! the Box's thread. A new watch connection replays every event, so the feed folds afresh and
//! acts only once the backlog is caught up. Each status message is guarded by its `slack.db`
//! row, so a refold after a reconnect or a restart posts none twice.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moor_client::WatchItem;
use moor_core::name::BoxId;
use moor_core::secret::Secret;

use crate::api::Api;
use crate::db::Db;
use crate::dial::Watched;
use crate::Log;

use status::{Planner, Step, View};

/// Longest the feed waits for a watch item before it acts anyway, which sends the status
/// edits the per-Box window held back.
const FEED_TICK: Duration = Duration::from_millis(500);

/// What the feed folded from one connection's events.
#[derive(Default)]
struct Fold {
    /// Each Box's status, by Box id.
    views: HashMap<BoxId, View>,
    /// Whether this connection's backlog is folded.
    caught_up: bool,
}

impl Fold {
    /// Folds one item; a new connection starts a fresh fold.
    fn take(&mut self, watched: Watched) {
        let event = match watched {
            Watched::Connected => return *self = Self::default(),
            Watched::Item(WatchItem::CaughtUp { .. }) => return self.caught_up = true,
            Watched::Item(WatchItem::Event(event)) => event,
        };
        if let Some(id) = event.box_id {
            self.views
                .entry(id)
                .or_default()
                .fold(&event.kind, &event.payload);
        }
    }
}

/// The feed thread's state: where `slack.db` lives and Slack.
pub(crate) struct Feed {
    /// Path of `slack.db`; the feed opens its own connection.
    pub(crate) db_path: PathBuf,
    /// Web API client.
    pub(crate) api: Arc<Api>,
    /// Bot token for posting.
    pub(crate) bot_token: Arc<Secret>,
    /// Daemon log.
    pub(crate) log: Log,
}

impl Feed {
    /// Folds watch items as they arrive and acts after each batch, and at least every
    /// [`FEED_TICK`], until the watch reader is gone.
    pub(crate) fn run(&self, items: &Receiver<Watched>) {
        let db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => return (self.log)(&format!("open slack.db: {e}")),
        };
        let mut fold = Fold::default();
        let mut planner = Planner::default();
        loop {
            match items.recv_timeout(FEED_TICK) {
                Ok(watched) => {
                    fold.take(watched);
                    for watched in items.try_iter() {
                        fold.take(watched);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if fold.caught_up {
                self.act(&db, &fold, &mut planner);
            }
        }
    }

    /// Sends the status messages that are due.
    fn act(&self, db: &Db, fold: &Fold, planner: &mut Planner) {
        let boxes = match db.fed_boxes() {
            Ok(boxes) => boxes,
            Err(e) => return (self.log)(&format!("slack.db: {e}; reading again")),
        };
        let at = now();
        for step in planner.plan(&boxes, &fold.views, at) {
            self.send_status(db, &step);
            planner.record(&step, at);
        }
    }

    /// Posts or edits one status message and records what it shows.
    fn send_status(&self, db: &Db, step: &Step) {
        let bot = &self.bot_token;
        let sent = step.ts.as_ref().map_or_else(
            || {
                self.api
                    .post_message(bot, &step.channel, &step.thread_ts, &step.text)
                    .map_err(|e| format!("slack: status post failed: {e}"))
            },
            |ts| {
                self.api
                    .update_message(bot, &step.channel, ts, &step.text)
                    .map(|()| ts.clone())
                    .map_err(|e| format!("slack: status edit failed: {e}"))
            },
        );
        let recorded = sent.and_then(|ts| {
            db.set_status(&step.box_name, &ts, &step.text)
                .map_err(|e| format!("slack.db: {e}"))
        });
        if let Err(why) = recorded {
            (self.log)(&why);
        }
    }
}

/// The current instant; the feed waits with `recv_timeout`, which runs on real time.
#[allow(
    clippy::disallowed_methods,
    reason = "the feed waits with recv_timeout, which runs on real time"
)]
fn now() -> Instant {
    Instant::now()
}

mod status;

#[cfg(test)]
mod tests;
