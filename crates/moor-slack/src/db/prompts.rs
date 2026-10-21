//! Each Box's needs-input prompts: one row per question the front posted in the Box's
//! thread, keyed by the Box and the event that asked it, so a refold posts none twice. A
//! prompt is open until someone answers it or the Box moves on.

use moor_core::name::BoxName;
use rusqlite::types::Type;
use rusqlite::{params, OptionalExtension, Row};

use super::Db;

/// Columns of a prompt row joined to its Box, in the order [`prompt_row`] reads them.
const PROMPT_COLUMNS: &str = "p.id, p.event_id, p.box_name, b.channel, b.thread_ts, \
     b.requested_by, p.message_ts, p.question, p.options, p.answer, p.answered_by, \
     p.closed_reason FROM slack_prompts p JOIN slack_boxes b ON b.box_name = p.box_name";

/// Why a prompt Slack refused to post is closed.
const REFUSED: &str = "not posted (Slack refused the message)";

/// Where a prompt stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromptState {
    /// Waiting for an answer.
    Open,
    /// Answered.
    Answered {
        /// The answer as the message shows it, such as `2. sqlite`.
        label: String,
        /// Slack user id of the person who answered.
        by: String,
    },
    /// The Box moved on before anyone answered; the text says what it is now.
    Closed(String),
}

/// A question a Box asked, as its thread shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prompt {
    /// The front's id for the prompt.
    pub(crate) id: i64,
    /// Cursor of the `needs_input` event that asked it.
    pub(crate) event_id: i64,
    /// The Box that asked.
    pub(crate) box_name: BoxName,
    /// Channel id of the Box's thread.
    pub(crate) channel: String,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: String,
    /// Slack user id of the person who asked for the Box; only they may answer.
    pub(crate) requested_by: String,
    /// Ts of the prompt message, `None` when Slack refused to post it.
    pub(crate) message_ts: Option<String>,
    /// The question, empty when the Box gave none.
    pub(crate) question: String,
    /// The options offered, numbered from 1.
    pub(crate) options: Vec<String>,
    /// Where it stands.
    pub(crate) state: PromptState,
}

/// A prompt message just posted, before it has a row.
pub(crate) struct Posted<'a> {
    /// The Box that asked.
    pub(crate) box_name: &'a BoxName,
    /// Cursor of the `needs_input` event.
    pub(crate) event_id: i64,
    /// Ts of the message, `None` when Slack refused it; such a prompt is recorded closed.
    pub(crate) message_ts: Option<&'a str>,
    /// The question.
    pub(crate) question: &'a str,
    /// The options offered.
    pub(crate) options: &'a [String],
}

impl Db {
    /// Whether Box `name` already has a prompt for event `event_id`.
    pub(crate) fn has_prompt(&self, name: &BoxName, event_id: i64) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT count(*) > 0 FROM slack_prompts WHERE box_name = ?1 AND event_id = ?2",
            params![name.as_str(), event_id],
            |r| r.get(0),
        )
    }

    /// Records the prompt message `posted` at `now`, once per Box and event. A prompt Slack
    /// refused is recorded closed, so it is not posted again.
    pub(crate) fn insert_prompt(&self, posted: &Posted<'_>, now: i64) -> rusqlite::Result<()> {
        let options = serde_json::to_string(posted.options)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
        self.conn.execute(
            "INSERT OR IGNORE INTO slack_prompts \
             (box_name, event_id, message_ts, question, options, created_at, closed_reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, CASE WHEN ?3 IS NULL THEN ?7 END)",
            params![
                posted.box_name.as_str(),
                posted.event_id,
                posted.message_ts,
                posted.question,
                options,
                now,
                REFUSED
            ],
        )?;
        Ok(())
    }

    /// The prompt whose message is `message_ts` in `channel`, if any.
    pub(crate) fn prompt_by_message(
        &self,
        channel: &str,
        message_ts: &str,
    ) -> rusqlite::Result<Option<Prompt>> {
        let sql = format!("SELECT {PROMPT_COLUMNS} WHERE b.channel = ?1 AND p.message_ts = ?2");
        self.conn
            .query_row(&sql, params![channel, message_ts], prompt_row)
            .optional()
    }

    /// The prompt with id `id`, if any.
    pub(crate) fn prompt(&self, id: i64) -> rusqlite::Result<Option<Prompt>> {
        let sql = format!("SELECT {PROMPT_COLUMNS} WHERE p.id = ?1");
        self.conn.query_row(&sql, [id], prompt_row).optional()
    }

    /// The open prompts of a thread's Boxes, oldest first.
    pub(crate) fn open_prompts(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> rusqlite::Result<Vec<Prompt>> {
        self.prompts(
            "b.channel = ?1 AND b.thread_ts = ?2 AND p.answer IS NULL \
             AND p.closed_reason IS NULL",
            params![channel, thread_ts],
        )
    }

    /// The open prompts of Box `name`, oldest first.
    pub(crate) fn open_box_prompts(&self, name: &BoxName) -> rusqlite::Result<Vec<Prompt>> {
        self.prompts(
            "p.box_name = ?1 AND p.answer IS NULL AND p.closed_reason IS NULL",
            params![name.as_str()],
        )
    }

    /// The answered prompts of Box `name` whose message does not show the answer yet.
    pub(crate) fn unshown_answers(&self, name: &BoxName) -> rusqlite::Result<Vec<Prompt>> {
        self.prompts(
            "p.box_name = ?1 AND p.answer IS NOT NULL AND p.shown_at IS NULL",
            params![name.as_str()],
        )
    }

    /// Records that `by` answered open prompt `id` with `label`; false when it was no longer
    /// open.
    pub(crate) fn answer_prompt(&self, id: i64, label: &str, by: &str) -> rusqlite::Result<bool> {
        let changed = self.conn.execute(
            "UPDATE slack_prompts SET answer = ?2, answered_by = ?3 \
             WHERE id = ?1 AND answer IS NULL AND closed_reason IS NULL",
            params![id, label, by],
        )?;
        Ok(changed == 1)
    }

    /// Closes open prompt `id` because its Box is now `reason`.
    pub(crate) fn close_prompt(&self, id: i64, reason: &str) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "UPDATE slack_prompts SET closed_reason = ?2 \
                 WHERE id = ?1 AND answer IS NULL AND closed_reason IS NULL",
                params![id, reason],
            )
            .map(drop)
    }

    /// Records at `now` that prompt `id`'s message shows its answer.
    pub(crate) fn mark_answer_shown(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "UPDATE slack_prompts SET shown_at = ?2 WHERE id = ?1",
                params![id, now],
            )
            .map(drop)
    }

    /// The prompts that match `filter`, oldest first.
    fn prompts(&self, filter: &str, args: impl rusqlite::Params) -> rusqlite::Result<Vec<Prompt>> {
        let sql = format!("SELECT {PROMPT_COLUMNS} WHERE {filter} ORDER BY p.id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(args, prompt_row)?;
        rows.collect()
    }
}

/// Reads one prompt row selected with [`PROMPT_COLUMNS`].
fn prompt_row(r: &Row<'_>) -> rusqlite::Result<Prompt> {
    let bad = |col: usize, why: String| {
        rusqlite::Error::FromSqlConversionFailure(col, Type::Text, why.into())
    };
    let name: String = r.get(2)?;
    let options: String = r.get(8)?;
    let answer: Option<String> = r.get(9)?;
    let by: Option<String> = r.get(10)?;
    let closed: Option<String> = r.get(11)?;
    let state = match (answer, by, closed) {
        (None, None, None) => PromptState::Open,
        (Some(label), Some(by), None) => PromptState::Answered { label, by },
        (None, None, Some(reason)) => PromptState::Closed(reason),
        _ => return Err(bad(9, "a prompt both answered and closed".to_owned())),
    };
    Ok(Prompt {
        id: r.get(0)?,
        event_id: r.get(1)?,
        box_name: BoxName::parse_allow_bare(&name).map_err(|e| bad(2, e.to_string()))?,
        channel: r.get(3)?,
        thread_ts: r.get(4)?,
        requested_by: r.get(5)?,
        message_ts: r.get(6)?,
        question: r.get(7)?,
        options: serde_json::from_str(&options).map_err(|e| bad(8, e.to_string()))?,
        state,
    })
}

#[cfg(test)]
mod tests;
