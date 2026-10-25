//! The feed: folds the controller's events by Box id and keeps each Box's status message,
//! its fixed lines, its needs-input prompts and its published artifact versions in the Box's
//! thread. A new watch connection replays every event, so the feed folds afresh and acts only
//! once the backlog is caught up.
//! Every side effect is guarded by a `slack.db` row, so a refold after a reconnect or a
//! restart repeats none.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moor_client::WatchItem;
use moor_core::name::BoxId;
use moor_core::secret::Secret;
use moor_core::user::Identity;
use moor_core::view::ArtifactView;

use crate::api::Api;
use crate::command::{closed, finished, BoxEvent, Stop};
use crate::db::{BoxState, Db, FedBox, SlackBox};
use crate::dial::{Tagged, Watched};
use crate::identity::Link;
use crate::watchers::Watchers;
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
    /// Each Box's session that ended without a clean exit and whose turn has not yet gone idle.
    stopping: HashMap<BoxId, Stop>,
    /// Boxes whose transcript could not be saved before their session end was seen.
    transcript_lost: HashSet<BoxId>,
    /// Each Box's unexpected session ends, by cursor of the idle that followed, whose line is
    /// not yet known to be posted.
    stopped: HashMap<BoxId, Vec<(i64, Stop)>>,
    /// Boxes that closed and whose closed line is not yet known to be posted.
    closed: HashSet<BoxId>,
    /// The question each Box waits on.
    asked: HashMap<BoxId, prompts::Asked>,
    /// What happened to each Box that its thread is owed a line for, by cursor.
    notes: HashMap<BoxId, Vec<(i64, lifecycle::Note)>>,
    /// Artifact versions each Box published that are not yet known to be shown.
    published: HashMap<BoxId, Vec<ArtifactView>>,
    /// The cursor of the event that began each working Box's current turn.
    turns: HashMap<BoxId, i64>,
    /// The name the controller gives each Box.
    names: HashMap<BoxId, String>,
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
        if let Some(name) = &event.box_name {
            self.names.insert(id, name.clone());
        }
        let view = self.views.entry(id).or_default();
        let was_working = view.is_working();
        view.fold(&event.kind, &event.payload, event.ts);
        match (was_working, view.is_working()) {
            (false, true) => drop(self.turns.insert(id, event.cursor)),
            (_, false) => drop(self.turns.remove(&id)),
            (true, true) => {}
        }
        prompts::fold(self, id, &event.kind, event.cursor, &event.payload);
        artifacts::fold(self, id, &event.kind, &event.payload);
        lifecycle::fold(self, id, event.cursor, &event.kind, &event.payload);
        stopped::fold(self, id, &event.kind, &event.payload);
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
                if let Some(stop) = self.stopping.remove(&id) {
                    self.stopped
                        .entry(id)
                        .or_default()
                        .push((event.cursor, stop));
                    return;
                }
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

/// The fold of each Slack user's watch. A connection resets only its own user's fold.
#[derive(Default)]
struct Folds(HashMap<Identity, Fold>);

impl Folds {
    /// Folds one item into the fold of the user whose watch sent it.
    fn take(&mut self, (identity, watched): Tagged) {
        self.0.entry(identity).or_default().take(watched);
    }

    /// The folds whose backlog is folded, so the feed may act on them.
    fn caught_up(&mut self) -> impl Iterator<Item = (&Identity, &mut Fold)> {
        self.0.iter_mut().filter(|(_, fold)| fold.caught_up)
    }
}

/// The feed thread's state: where `slack.db` lives, Slack and the controller.
pub(crate) struct Feed {
    /// The controller and team; the feed follows one watch per Slack user.
    pub(crate) link: Link,
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
    /// Seconds of one turn after which, and after each further so many, the thread gets a line.
    pub(crate) long_turn_secs: i64,
}

impl Feed {
    /// Follows one watch per thread owner, folds watch items as they arrive and acts after each
    /// batch, and at least every [`FEED_TICK`], until the process ends.
    pub(crate) fn run(&self) {
        let db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => return (self.log)(&format!("open slack.db: {e}")),
        };
        let (events, items) = mpsc::channel();
        let mut watchers = Watchers::new(self.link.clone(), events, Arc::clone(&self.log));
        let mut folds = Folds::default();
        let mut planner = Planner::default();
        let mut lines_due = now();
        loop {
            match db.thread_owners() {
                Ok(owners) => watchers.follow(owners),
                Err(e) => (self.log)(&format!("slack.db: {e}; reading again")),
            }
            match items.recv_timeout(FEED_TICK) {
                Ok(tagged) => {
                    folds.take(tagged);
                    for tagged in items.try_iter() {
                        folds.take(tagged);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.act(&db, &mut folds, &mut planner, &mut lines_due);
        }
    }

    /// For each user whose watch is caught up: sends the status messages that are due, closes
    /// the prompts whose Box moved on, then sends the lines and prompts not yet posted. A line
    /// or prompt that fails holds back every line and prompt for [`UPDATE_EVERY`].
    fn act(&self, db: &Db, folds: &mut Folds, planner: &mut Planner, lines_due: &mut Instant) {
        let boxes = match db.fed_boxes() {
            Ok(boxes) => boxes,
            Err(e) => return (self.log)(&format!("slack.db: {e}; reading again")),
        };
        let mut owned: HashMap<Identity, Vec<FedBox>> = HashMap::new();
        for fed in boxes {
            if let Ok(owner) = self.link.identity(&fed.boxed.requested_by) {
                owned.entry(owner).or_default().push(fed);
            }
        }
        let at = now();
        for (identity, fold) in folds.caught_up() {
            let Some(boxes) = owned.remove(identity) else {
                continue;
            };
            let boxes = self.current(db, fold, boxes);
            for step in planner.plan(&boxes, &fold.views, at, (self.clock)()) {
                self.send_status(db, &step);
                planner.record(&step, at);
            }
            for fed in &boxes {
                self.close_moved_on(db, fold, &fed.boxed);
            }
            if at < *lines_due {
                continue;
            }
            for fed in &boxes {
                if !self.lines(db, fold, &fed.boxed)
                    || !self.stops(db, fold, &fed.boxed)
                    || !self.notes(db, fold, &fed.boxed)
                    || !self.prompts(db, fold, &fed.boxed)
                    || !self.artifacts(db, fold, &fed.boxed)
                    || !self.long_turn(db, fold, &fed.boxed)
                {
                    *lines_due = at + UPDATE_EVERY;
                    return;
                }
            }
        }
    }

    /// The rows of `boxes` whose id the controller gives no other Box. A recreated controller
    /// store hands its ids out again, so an older row can share its id with a different Box.
    /// Such a bound row is marked gone, like a Box the controller no longer has.
    fn current(&self, db: &Db, fold: &Fold, boxes: Vec<FedBox>) -> Vec<FedBox> {
        let mut current = Vec::with_capacity(boxes.len());
        for fed in boxes {
            let row = &fed.boxed;
            let id = prompts::box_id(&row.state);
            let Some((id, named)) = id.and_then(|id| Some((id, fold.names.get(&id)?))) else {
                current.push(fed);
                continue;
            };
            if named == row.box_name.as_str() {
                current.push(fed);
                continue;
            }
            if !matches!(row.state, BoxState::Bound { .. }) {
                continue;
            }
            let name = row.box_name.as_str();
            match db.mark_gone(&row.box_name) {
                Ok(()) => (self.log)(&format!(
                    "feed: Box {name} is gone; the controller's Box {} is now {named}",
                    id.get()
                )),
                Err(e) => (self.log)(&format!("slack.db: {e}")),
            }
        }
        current
    }

    /// Posts or edits one status message and records what it shows.
    fn send_status(&self, db: &Db, step: &Step) {
        let bot = &self.bot_token;
        let sent = step.ts.as_ref().map_or_else(
            || {
                self.api
                    .post_blocks(
                        bot,
                        &step.channel,
                        &step.thread_ts,
                        &step.text,
                        &step.blocks,
                    )
                    .map_err(|e| format!("slack: status post failed: {e}"))
            },
            |ts| {
                self.api
                    .update_blocks(bot, &step.channel, ts, &step.text, &step.blocks)
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

    /// Posts the finished and closed lines of `boxed` that are not yet posted, and marks a
    /// bound Box the controller closed gone. A Box that is closing gets its closed line and
    /// its `gone` from the worker's close path. Returns false when a line could not be posted
    /// or recorded.
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
            if let Err(e) = db.mark_gone(name) {
                (self.log)(&format!("slack.db: {e}"));
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

mod artifacts;
mod lifecycle;
mod long_turn;
mod prompts;
mod status;
mod stopped;

#[cfg(test)]
mod tests;
