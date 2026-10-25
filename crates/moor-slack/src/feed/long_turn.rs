//! One plain line in a Box's thread each time its turn has run another `long_turn_minutes`,
//! so the requester is notified, not only shown an edited status. The line is keyed by the
//! Box, the event that began the turn and the threshold in `slack.db`, so a refold or a
//! restart posts none twice, and a turn that ends posts no further line.

use moor_core::name::{BoxId, BoxName};

use super::prompts::box_id;
use super::status::{minutes, State, View};
use super::{Feed, Fold};
use crate::command::BoxEvent;
use crate::db::{Db, SlackBox};
use crate::escape::code;

/// Most thresholds one turn can key; the cursor of its first event is scaled past them.
const THRESHOLDS: i64 = 1000;

/// The threshold a turn of `view` has reached at unix second `now`, counted from 1: the turn
/// has run `n` whole `every_secs`. None while the Box is not working or is under the first.
fn reached(view: &View, now: i64, every_secs: i64) -> Option<i64> {
    if view.state != State::Working {
        return None;
    }
    let ran = now - view.since_ms?.div_euclid(1000);
    let n = (ran / every_secs.max(1)).min(THRESHOLDS - 1);
    (n >= 1).then_some(n)
}

/// The notice key of threshold `n` of the turn that began at event `cursor`.
const fn key(cursor: i64, n: i64) -> i64 {
    cursor * THRESHOLDS + n
}

/// The line for Box `name` after `n` thresholds of `every_secs`, with what the agent is doing.
fn line(name: &BoxName, doing: Option<&str>, n: i64, every_secs: i64) -> String {
    let name = name.as_str();
    let now = doing.map_or_else(String::new, |d| format!(" Now: {}.", code(d)));
    format!(
        "`{name}` has been working for {}.{now} Reply here to redirect it, or ask me to stop it.",
        minutes(n * every_secs)
    )
}

impl Feed {
    /// Posts the long-turn line of `boxed` when its turn has reached a threshold not yet
    /// posted. Returns false when the line could not be posted or recorded.
    pub(super) fn long_turn(&self, db: &Db, fold: &Fold, boxed: &SlackBox) -> bool {
        let Some(id): Option<BoxId> = box_id(&boxed.state) else {
            return true;
        };
        let (Some(view), Some(turn)) = (fold.views.get(&id), fold.turns.get(&id)) else {
            return true;
        };
        let Some(n) = reached(view, (self.clock)(), self.long_turn_secs) else {
            return true;
        };
        let text = line(
            &boxed.box_name,
            view.doing.as_deref(),
            n,
            self.long_turn_secs,
        );
        let key = (BoxEvent::LongTurn, Some(key(*turn, n)));
        let queue = || db.notice(boxed, BoxEvent::LongTurn, key.1);
        self.line(db, boxed, key, &text, queue)
    }
}

#[cfg(test)]
mod tests;
