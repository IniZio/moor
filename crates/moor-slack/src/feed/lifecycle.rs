//! One plain line in a Box's thread for a launch that failed or timed out, a sleep the timer
//! forced, an automatic agent relaunch and a relaunch the controller refused. Each ends with
//! how to carry on by replying. The line is keyed by its Box, kind and event cursor in
//! `slack.db`, so a refold or a restart posts none twice, and the same key queues a fixed
//! notice for the thread's next turn so the coordinator knows what the line is about.

use moor_core::name::{BoxId, BoxName};
use serde_json::Value;

use super::prompts::box_id;
use super::{Feed, Fold};
use crate::command::BoxEvent;
use crate::db::{Db, SlackBox};

/// What happened to a Box that its thread should be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Note {
    /// The agent launch failed, so no session started.
    LaunchFailed,
    /// The agent did not start within the launch limit, in seconds.
    LaunchTimedOut(u64),
    /// The sleep timer put the Box to sleep.
    SleptIdle,
    /// The agent was started again on its own after it ended.
    Relaunched,
    /// The controller refused another automatic relaunch: relaunches seen, window in seconds.
    RelaunchCapped(u64, u64),
}

impl Note {
    /// The note the event of `kind` is, if any. A relaunch with no cause was asked for.
    fn of(kind: &str, payload: &Value) -> Option<Self> {
        let number = |key: &str| payload.get(key).and_then(Value::as_u64).unwrap_or(0);
        match kind {
            "session_failed" => Some(Self::LaunchFailed),
            "launch_timed_out" => Some(Self::LaunchTimedOut(number("after_secs"))),
            "asleep" if payload.get("auto").and_then(Value::as_bool) == Some(true) => {
                Some(Self::SleptIdle)
            }
            "relaunched" if payload.get("cause").is_some_and(|c| !c.is_null()) => {
                Some(Self::Relaunched)
            }
            "relaunch_capped" => Some(Self::RelaunchCapped(number("count"), number("window_secs"))),
            _ => None,
        }
    }

    /// The Box event whose notice and dedup key this note uses.
    pub(super) const fn event(&self) -> BoxEvent {
        match self {
            Self::LaunchFailed => BoxEvent::LaunchFailed,
            Self::LaunchTimedOut(_) => BoxEvent::LaunchTimedOut,
            Self::SleptIdle => BoxEvent::SleptIdle,
            Self::Relaunched => BoxEvent::Relaunched,
            Self::RelaunchCapped(..) => BoxEvent::RelaunchCapped,
        }
    }

    /// The line for Box `name`; it carries no text from a Box or the controller.
    pub(super) fn line(&self, name: &BoxName) -> String {
        let name = name.as_str();
        match self {
            Self::LaunchFailed => format!(
                "Box `{name}` could not start its agent. Reply here and ask me to restart the agent."
            ),
            Self::LaunchTimedOut(secs) => format!(
                "Box `{name}` did not start its agent within {}, so it may go to sleep. Reply here and ask me to restart the agent.",
                span(*secs)
            ),
            Self::SleptIdle => {
                format!("Box `{name}` went to sleep after sitting idle. Reply here to wake it.")
            }
            Self::Relaunched => format!(
                "Box `{name}`'s agent had ended, so it was started again. If it stalls, reply here and ask me to restart the agent."
            ),
            Self::RelaunchCapped(count, secs) => format!(
                "Box `{name}`'s agent ended {count} times in the last {}, so it is not started again on its own. Reply here and ask me to restart the agent once the cause is fixed.",
                span(*secs)
            ),
        }
    }
}

/// `secs` as `3m` when it is whole minutes, else `90s`.
fn span(secs: u64) -> String {
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Applies one event of Box `id`: each noteworthy event owes the thread its line.
pub(super) fn fold(fold: &mut Fold, id: BoxId, cursor: i64, kind: &str, payload: &Value) {
    if let Some(note) = Note::of(kind, payload) {
        fold.notes.entry(id).or_default().push((cursor, note));
    }
}

impl Feed {
    /// Posts the lines of `boxed` that are not yet posted. Returns false when a line could not
    /// be posted or recorded.
    pub(super) fn notes(&self, db: &Db, fold: &mut Fold, boxed: &SlackBox) -> bool {
        let Some(id) = box_id(&boxed.state) else {
            return true;
        };
        let Some(owed) = fold.notes.get_mut(&id) else {
            return true;
        };
        while let Some((cursor, note)) = owed.first() {
            let key = (note.event(), Some(*cursor));
            let queue = || db.notice(boxed, note.event(), Some(*cursor));
            if !self.line(db, boxed, key, &note.line(&boxed.box_name), queue) {
                return false;
            }
            owed.remove(0);
        }
        true
    }
}
