//! The line a Box's thread gets when its agent session ended without a clean exit, in place of
//! the finished-turn line. It is posted as soon as the session end is seen, unless a later session
//! has replaced it, and is keyed by the session end's cursor so a refold or a restart posts none
//! twice. The same key queues a report that tells the coordinator the same.

use moor_core::name::BoxId;
use serde_json::Value;

use super::prompts::box_id;
use super::{Feed, Fold};
use crate::command::{BoxEvent, Stop};
use crate::db::{Db, SlackBox};

/// Applies one event of Box `id`. An agent that exited with code 0 ended cleanly, and a
/// session moor ended itself (a relaunch or a request) is not a stop. A Box that could not be
/// reached ends its session whatever the pane's exit code was. A new session replaces the stop
/// of an older one that is not yet posted.
pub(super) fn fold(fold: &mut Fold, id: BoxId, cursor: i64, kind: &str, payload: &Value) {
    match kind {
        "agent_unreachable" => drop(fold.unreachable.insert(id)),
        "session_ended" => {
            let exit = payload
                .get("exit")
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok());
            let lost = fold.transcript_lost.remove(&id);
            let unreachable = fold.unreachable.remove(&id);
            let exited = payload.get("cause").and_then(Value::as_str) == Some("exited");
            if exited && (unreachable || exit != Some(0)) {
                let stop = Stop {
                    exit,
                    transcript_lost: lost,
                    unreachable,
                };
                fold.stopping.insert(id);
                fold.stopped.entry(id).or_default().push((cursor, stop));
            }
        }
        "transcript_failed" => {
            let pending = fold.stopped.get_mut(&id).and_then(|owed| owed.last_mut());
            match pending {
                Some((_, stop)) => stop.transcript_lost = true,
                None => drop(fold.transcript_lost.insert(id)),
            }
        }
        "session_started" | "relaunched" => {
            fold.stopped.remove(&id);
            fold.stopping.remove(&id);
            fold.unreachable.remove(&id);
            fold.transcript_lost.remove(&id);
        }
        "working" | "busy" | "woke" => {
            fold.transcript_lost.remove(&id);
        }
        _ => {}
    }
}

impl Feed {
    /// Posts the unexpected-end lines of `boxed` that are not yet posted. Returns false when a
    /// line could not be posted or recorded.
    pub(super) fn stops(&self, db: &Db, fold: &mut Fold, boxed: &SlackBox) -> bool {
        let Some(id) = box_id(&boxed.state) else {
            return true;
        };
        let Some(owed) = fold.stopped.get_mut(&id) else {
            return true;
        };
        while let Some((cursor, stop)) = owed.first() {
            let key = (BoxEvent::Stopped(*stop), Some(*cursor));
            let queue = || db.stopped(boxed, *cursor, *stop);
            if !self.line(db, boxed, key, &stop.line(boxed.box_name.as_str()), queue) {
                return false;
            }
            owed.remove(0);
        }
        true
    }
}
