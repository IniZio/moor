//! The line a Box's thread gets when its agent session ended without a clean exit, in place of
//! the finished-turn line. The session end and the idle that follows it are folded together,
//! so the line is keyed by the idle's cursor like a finished turn and a refold or a restart
//! posts none twice. The same key queues a report that tells the coordinator the same.

use moor_core::name::BoxId;
use serde_json::Value;

use super::prompts::box_id;
use super::{Feed, Fold};
use crate::command::{BoxEvent, Stop};
use crate::db::{Db, SlackBox};

/// Applies one event of Box `id`. An agent that exited with code 0 ended cleanly, and a
/// session moor ended itself (a relaunch or a request) is not a stop.
pub(super) fn fold(fold: &mut Fold, id: BoxId, kind: &str, payload: &Value) {
    match kind {
        "session_ended" => {
            let exit = payload
                .get("exit")
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok());
            let lost = fold.transcript_lost.remove(&id);
            if payload.get("cause").and_then(Value::as_str) == Some("exited") && exit != Some(0) {
                let stop = Stop {
                    exit,
                    transcript_lost: lost,
                };
                fold.stopping.insert(id, stop);
            }
        }
        "transcript_failed" => match fold.stopping.get_mut(&id) {
            Some(stop) => stop.transcript_lost = true,
            None => drop(fold.transcript_lost.insert(id)),
        },
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
