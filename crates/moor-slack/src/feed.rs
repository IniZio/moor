//! The feed: folds the controller's events by Box id and keeps each Box's status message and
//! its fixed lines in the Box's thread. A new watch connection replays every event, so the
//! feed folds afresh and acts only once the backlog is caught up. Every side effect is
//! guarded by a `slack.db` row, so a refold after a reconnect or a restart repeats none.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moor_client::WatchItem;
use moor_core::name::BoxId;
use moor_core::secret::Secret;

use crate::api::Api;
use crate::command::{closed, finished, BoxEvent};
use crate::db::{BoxState, Db, SlackBox};
use crate::dial::Watched;
use crate::Log;

use status::{Planner, Step, View, UPDATE_EVERY};

/// Longest the feed waits for a watch item before it acts anyway, which sends the status
/// edits the per-Box window held back.
const FEED_TICK: Duration = Duration::from_millis(500);

/// What the feed folded from one connection's events.
#[derive(Default)]
struct Fold {
    /// Each Box's status, by Box id.
    views: HashMap<BoxId, View>,
    /// Cursors of each Box's finished turns whose line is not yet known to be posted.
    finished: HashMap<BoxId, Vec<i64>>,
    /// Boxes that closed and whose closed line is not yet known to be posted.
    closed: HashSet<BoxId>,
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
        let Some(id) = event.box_id else {
            return;
        };
        self.views
            .entry(id)
            .or_default()
            .fold(&event.kind, &event.payload);
        match event.kind.as_str() {
            "idled" => self.finished.entry(id).or_default().push(event.cursor),
            "closed" => drop(self.closed.insert(id)),
            _ => {}
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
        let mut lines_due = now();
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
                self.act(&db, &mut fold, &mut planner, &mut lines_due);
            }
        }
    }

    /// Sends the status messages that are due, then the lines not yet posted. A line that
    /// fails holds back every line for [`UPDATE_EVERY`].
    fn act(&self, db: &Db, fold: &mut Fold, planner: &mut Planner, lines_due: &mut Instant) {
        let boxes = match db.fed_boxes() {
            Ok(boxes) => boxes,
            Err(e) => return (self.log)(&format!("slack.db: {e}; reading again")),
        };
        let at = now();
        for step in planner.plan(&boxes, &fold.views, at) {
            self.send_status(db, &step);
            planner.record(&step, at);
        }
        if at < *lines_due {
            return;
        }
        for fed in &boxes {
            if !self.lines(db, fold, &fed.boxed) {
                *lines_due = at + UPDATE_EVERY;
                return;
            }
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

    /// Posts the finished and closed lines of `boxed` that are not yet posted. A Box that is
    /// closing gets its closed line from the worker's close path. Returns false when a line
    /// could not be posted or recorded.
    fn lines(&self, db: &Db, fold: &mut Fold, boxed: &SlackBox) -> bool {
        let (BoxState::Bound { box_id, .. }
        | BoxState::Closing { box_id, .. }
        | BoxState::Gone { box_id, .. }) = boxed.state
        else {
            return true;
        };
        let name = &boxed.box_name;
        if let Some(cursors) = fold.finished.get_mut(&box_id) {
            while let Some(&cursor) = cursors.first() {
                if !self.line(db, boxed, BoxEvent::Finished, Some(cursor), &finished(name)) {
                    return false;
                }
                cursors.remove(0);
            }
        }
        if fold.closed.contains(&box_id) && !matches!(boxed.state, BoxState::Closing { .. }) {
            if !self.line(db, boxed, BoxEvent::Closed, None, &closed(name)) {
                return false;
            }
            fold.closed.remove(&box_id);
        }
        true
    }

    /// Posts `text` in the thread of `boxed` and then queues the notice of `event`, unless
    /// the notice is already queued. Posting first means a crash in between posts the line
    /// again instead of losing it. Returns whether the notice is queued.
    fn line(
        &self,
        db: &Db,
        boxed: &SlackBox,
        event: BoxEvent,
        cursor: Option<i64>,
        text: &str,
    ) -> bool {
        let failed = |why: String| {
            (self.log)(&why);
            false
        };
        match db.has_notice(boxed, event, cursor) {
            Ok(true) => return true,
            Ok(false) => {}
            Err(e) => return failed(format!("slack.db: {e}")),
        }
        let (channel, thread) = (&boxed.channel, &boxed.thread_ts);
        if let Err(e) = self
            .api
            .post_message(&self.bot_token, channel, thread, text)
        {
            return failed(format!("slack: line failed: {e}"));
        }
        match db.notice(boxed, event, cursor) {
            Ok(_) => true,
            Err(e) => failed(format!("slack.db: {e}")),
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
