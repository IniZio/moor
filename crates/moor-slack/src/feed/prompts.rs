//! Each Box's needs-input prompts in its thread: the question the Box waits on, folded from
//! its events, gets one message with a button per option. The feed closes a prompt whose Box
//! moved on and edits an answered prompt's message to show the answer. `slack.db` keys each
//! prompt by its Box and event, so a refold posts none twice.

use moor_core::name::BoxId;
use serde_json::Value;

use super::status::{State, View};
use super::{Feed, Fold};
use crate::db::{BoxState, Db, Posted, Prompt, PromptState, SlackBox};
use crate::prompt::{answered_blocks, open_blocks, text};

/// The question a Box waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Asked {
    /// Cursor of the `needs_input` event.
    pub(super) cursor: i64,
    /// The question, empty when none.
    pub(super) question: String,
    /// The options offered.
    pub(super) options: Vec<String>,
}

/// Applies one event of Box `id` to the questions Boxes wait on: a `needs_input` opens (or
/// replaces) the Box's question, and any event that moves the Box on clears it.
pub(super) fn fold(fold: &mut Fold, id: BoxId, kind: &str, cursor: i64, payload: &Value) {
    match kind {
        "needs_input" => {
            let question = payload
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let options = payload
                .get("options")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            fold.asked.insert(
                id,
                Asked {
                    cursor,
                    question,
                    options,
                },
            );
        }
        "working" | "busy" | "woke" | "idled" | "asleep" | "closed" | "rejected"
        | "setup_failed" => drop(fold.asked.remove(&id)),
        _ => {}
    }
}

/// What a Box whose prompt `prompt` is open has moved on to, or `None` while the prompt is
/// still the question it waits on.
fn moved_on(
    prompt: &Prompt,
    row: &BoxState,
    view: Option<&View>,
    asked: Option<&Asked>,
) -> Option<&'static str> {
    if matches!(row, BoxState::Gone { .. }) {
        return Some("closed");
    }
    match asked {
        Some(asked) if asked.cursor == prompt.event_id => return None,
        Some(_) => return Some("waiting on a newer question"),
        None => {}
    }
    Some(match view.map(|v| &v.state) {
        None | Some(State::Working) => "working",
        Some(State::Starting) => "starting",
        Some(State::Idle) => "idle",
        Some(State::Asleep) => "asleep",
        Some(State::NeedsInput(_)) => "waiting on a newer question",
        Some(State::Closed) => "closed",
        Some(State::Rejected(_)) => "rejected",
        Some(State::SetupFailed(_)) => "failed setup",
    })
}

impl Feed {
    /// Closes each open prompt of `boxed` whose Box has moved on.
    pub(super) fn close_moved_on(&self, db: &Db, fold: &Fold, boxed: &SlackBox) {
        let Some(id) = box_id(&boxed.state) else {
            return;
        };
        let open = match db.open_box_prompts(&boxed.box_name) {
            Ok(open) => open,
            Err(e) => return (self.log)(&format!("slack.db: {e}")),
        };
        for prompt in open {
            let Some(reason) = moved_on(
                &prompt,
                &boxed.state,
                fold.views.get(&id),
                fold.asked.get(&id),
            ) else {
                continue;
            };
            if let Err(e) = db.close_prompt(prompt.id, reason) {
                (self.log)(&format!("slack.db: {e}"));
            }
        }
    }

    /// Shows each answer of `boxed` not yet shown, then posts the question its Box waits on
    /// when it has no prompt yet. Returns false when a message could not be sent or recorded.
    pub(super) fn prompts(&self, db: &Db, fold: &Fold, boxed: &SlackBox) -> bool {
        let failed = |why: String| {
            (self.log)(&why);
            false
        };
        let answered = match db.unshown_answers(&boxed.box_name) {
            Ok(answered) => answered,
            Err(e) => return failed(format!("slack.db: {e}")),
        };
        for prompt in &answered {
            if let Err(why) = self.show_answer(db, prompt) {
                return failed(why);
            }
        }
        let BoxState::Bound { box_id, .. } = boxed.state else {
            return true;
        };
        let Some(asked) = fold.asked.get(&box_id) else {
            return true;
        };
        match db.has_prompt(&boxed.box_name, asked.cursor) {
            Ok(false) => {}
            Ok(true) => return true,
            Err(e) => return failed(format!("slack.db: {e}")),
        }
        self.post(db, boxed, asked).map_or_else(failed, |()| true)
    }

    /// Posts the prompt for `asked` in the thread of `boxed`, then records it. A crash in
    /// between posts it again on the next refold.
    fn post(&self, db: &Db, boxed: &SlackBox, asked: &Asked) -> Result<(), String> {
        let name = boxed.box_name.as_str();
        let by = &boxed.requested_by;
        let blocks = open_blocks(name, by, &asked.question, &asked.options);
        let fallback = text(name, by, &asked.question);
        let (channel, thread) = (&boxed.channel, &boxed.thread_ts);
        let ts = self
            .api
            .post_blocks(&self.bot_token, channel, thread, &fallback, &blocks)
            .map_err(|e| format!("slack: prompt post failed: {e}"))?;
        let posted = Posted {
            box_name: &boxed.box_name,
            event_id: asked.cursor,
            message_ts: &ts,
            question: &asked.question,
            options: &asked.options,
        };
        db.insert_prompt(&posted, (self.clock)())
            .map_err(|e| format!("slack.db: {e}"))
    }

    /// Edits the message of the answered `prompt` to show who answered and how, without
    /// buttons, then records that it does.
    fn show_answer(&self, db: &Db, prompt: &Prompt) -> Result<(), String> {
        let PromptState::Answered { label, by } = &prompt.state else {
            return Ok(());
        };
        let name = prompt.box_name.as_str();
        let requester = &prompt.requested_by;
        let blocks = answered_blocks(name, requester, &prompt.question, label, by);
        let fallback = text(name, requester, &prompt.question);
        self.api
            .update_blocks(
                &self.bot_token,
                &prompt.channel,
                &prompt.message_ts,
                &fallback,
                &blocks,
            )
            .map_err(|e| format!("slack: prompt edit failed: {e}"))?;
        db.mark_answer_shown(prompt.id, (self.clock)())
            .map_err(|e| format!("slack.db: {e}"))
    }
}

/// The id of a Box the controller named.
const fn box_id(state: &BoxState) -> Option<BoxId> {
    match state {
        BoxState::Starting => None,
        BoxState::Bound { box_id, .. }
        | BoxState::Closing { box_id, .. }
        | BoxState::Gone { box_id, .. } => Some(*box_id),
    }
}

#[cfg(test)]
mod tests;
