//! The feed: folds the controller's events by Box id and keeps each Box's status message,
//! its fixed lines and its needs-input prompts in the Box's thread. A new watch connection
//! replays every event, so the feed folds afresh and acts only once the backlog is caught up.
//! Every side effect is guarded by a `slack.db` row, so a refold after a reconnect or a
//! restart repeats none.

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
    /// Each Box's finished turns, by cursor with the agent's last message, whose line is not
    /// yet known to be posted.
    finished: HashMap<BoxId, Vec<(i64, Option<String>)>>,
    /// The last message each Box's agent left in its current turn.
    said: HashMap<BoxId, String>,
    /// Boxes that closed and whose closed line is not yet known to be posted.
    closed: HashSet<BoxId>,
    /// The question each Box waits on.
    asked: HashMap<BoxId, prompts::Asked>,
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
        prompts::fold(self, id, &event.kind, event.cursor, &event.payload);
        match event.kind.as_str() {
            "agent_said" => {
                let text = event
                    .payload
                    .get("text")
                    .and_then(serde_json::Value::as_str);
                self.said.insert(id, text.unwrap_or_default().to_owned());
            }
            "working" | "busy" | "woke" => drop(self.said.remove(&id)),
            "idled" => {
                let said = self.said.remove(&id);
                self.finished
                    .entry(id)
                    .or_default()
                    .push((event.cursor, said));
            }
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
    /// The current time in unix seconds.
    pub(crate) clock: fn() -> i64,
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

    /// Sends the status messages that are due, closes the prompts whose Box moved on, then
    /// sends the lines and prompts not yet posted. A line or prompt that fails holds back
    /// every line and prompt for [`UPDATE_EVERY`].
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
        for fed in &boxes {
            self.close_moved_on(db, fold, &fed.boxed);
        }
        if at < *lines_due {
            return;
        }
        for fed in &boxes {
            if !self.lines(db, fold, &fed.boxed) || !self.prompts(db, fold, &fed.boxed) {
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
        if let Some(turns) = fold.finished.get_mut(&box_id) {
            while let Some((cursor, said)) = turns.first() {
                let queue = || db.report(boxed, *cursor, said.as_deref());
                let key = (BoxEvent::Finished, Some(*cursor));
                if !self.line(db, boxed, key, &finished(name), queue) {
                    return false;
                }
                turns.remove(0);
            }
        }
        if fold.closed.contains(&box_id) && !matches!(boxed.state, BoxState::Closing { .. }) {
            let queue = || db.notice(boxed, BoxEvent::Closed, None);
            if !self.line(db, boxed, (BoxEvent::Closed, None), &closed(name), queue) {
                return false;
            }
            fold.closed.remove(&box_id);
        }
        true
    }

    /// Posts `text` in the thread of `boxed` and then queues the item of `event` at `cursor`
    /// with `queue`, unless it is already queued. Posting first means a crash in between posts the
    /// line again instead of losing it. Returns whether the item is queued.
    fn line(
        &self,
        db: &Db,
        boxed: &SlackBox,
        (event, cursor): (BoxEvent, Option<i64>),
        text: &str,
        queue: impl FnOnce() -> rusqlite::Result<bool>,
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
        match queue() {
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

mod prompts;
mod status;

#[cfg(test)]
mod tests;
