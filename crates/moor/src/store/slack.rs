//! Slack event dedup and the thread-to-Task binding.

use moor_core::BoxName;
use rusqlite::{params, OptionalExtension};

use super::{parsed, Store, StoreError};

/// DDL for the Slack tables; idempotent so existing version-1 files gain them on open.
pub(super) const SLACK_SQL: &str = include_str!("slack.sql");

/// A Slack thread bound to a Task, with the user who started it and its status message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackThread {
    /// Channel id the thread lives in.
    pub(crate) channel: String,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: String,
    /// The Task the thread drives.
    pub(crate) task: BoxName,
    /// Slack user id of the Task owner.
    pub(crate) owner: String,
    /// Timestamp of the edited status message, once posted.
    pub(crate) status_ts: Option<String>,
}

/// A needs-input prompt message posted in a thread, keyed by the event that raised it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackPrompt {
    /// Id of the `needs_input` event row.
    pub(crate) event_id: i64,
    /// Channel id the prompt was posted in.
    pub(crate) channel: String,
    /// Root timestamp of the thread.
    pub(crate) thread_ts: String,
    /// Timestamp of the prompt message.
    pub(crate) message_ts: String,
    /// The Task that is waiting.
    pub(crate) task: BoxName,
    /// The question asked.
    pub(crate) question: String,
    /// The offered options, in order.
    pub(crate) options: Vec<String>,
    /// The recorded answer, once given.
    pub(crate) answer: Option<String>,
}

/// Columns read for a [`SlackPrompt`], in [`prompt_from_row`] order.
const PROMPT_COLS: &str =
    "event_id, channel, thread_ts, message_ts, task, question, options, answer";

/// Raw prompt row: the task and options are still unparsed text.
type PromptRow = (
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
);

/// Reads one raw prompt row.
fn prompt_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PromptRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
    ))
}

/// Parses a raw prompt row into a [`SlackPrompt`].
fn prompt_of(row: PromptRow) -> Result<SlackPrompt, StoreError> {
    let (event_id, channel, thread_ts, message_ts, task, question, options, answer) = row;
    Ok(SlackPrompt {
        event_id,
        channel,
        thread_ts,
        message_ts,
        task: parsed("slack prompt task", &task, BoxName::parse)?,
        question,
        options: serde_json::from_str(&options)?,
        answer,
    })
}

impl Store {
    /// Records a Slack event id; true the first time it is seen, false on redelivery.
    pub(crate) fn slack_event_once(&self, event_id: &str) -> Result<bool, StoreError> {
        let now = self.now();
        let added = self.writer()?.execute(
            "INSERT OR IGNORE INTO slack_events (event_id, seen_at) VALUES (?, ?)",
            params![event_id, now.as_millis()],
        )?;
        Ok(added == 1)
    }

    /// The Task bound to a Slack thread, if any.
    pub(crate) fn slack_thread(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> Result<Option<BoxName>, StoreError> {
        let raw: Option<String> = self
            .reader()?
            .query_row(
                "SELECT task FROM slack_threads WHERE channel = ? AND thread_ts = ?",
                params![channel, thread_ts],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|name| parsed("slack thread task", &name, BoxName::parse))
            .transpose()
    }

    /// The owner of the Task bound to a thread, if bound.
    pub(crate) fn slack_thread_owner(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> Result<Option<String>, StoreError> {
        Ok(self
            .reader()?
            .query_row(
                "SELECT owner FROM slack_threads WHERE channel = ? AND thread_ts = ?",
                params![channel, thread_ts],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// All bound threads, oldest first.
    pub(crate) fn slack_threads(&self) -> Result<Vec<SlackThread>, StoreError> {
        let reader = self.reader()?;
        let mut stmt = reader.prepare(
            "SELECT channel, thread_ts, task, owner, status_ts FROM slack_threads \
             ORDER BY created_at, rowid",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, String>(2)?,
                    r.get(3)?,
                    r.get(4)?,
                ))
            })?
            .collect::<Result<Vec<(String, String, String, String, Option<String>)>, _>>()?;
        rows.into_iter()
            .map(|(channel, thread_ts, task, owner, status_ts)| {
                Ok(SlackThread {
                    channel,
                    thread_ts,
                    task: parsed("slack thread task", &task, BoxName::parse)?,
                    owner,
                    status_ts,
                })
            })
            .collect()
    }

    /// Records the timestamp of a thread's status message.
    pub(crate) fn set_slack_status_ts(
        &self,
        channel: &str,
        thread_ts: &str,
        ts: &str,
    ) -> Result<(), StoreError> {
        self.writer()?.execute(
            "UPDATE slack_threads SET status_ts = ? WHERE channel = ? AND thread_ts = ?",
            params![ts, channel, thread_ts],
        )?;
        Ok(())
    }

    /// Binds a thread to a Task owned by `owner`; the first binding wins. True when this call created it.
    pub(crate) fn bind_slack_thread(
        &self,
        channel: &str,
        thread_ts: &str,
        task: &BoxName,
        owner: &str,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let added = self.writer()?.execute(
            "INSERT OR IGNORE INTO slack_threads (channel, thread_ts, task, owner, created_at) VALUES (?, ?, ?, ?, ?)",
            params![channel, thread_ts, task.as_str(), owner, now.as_millis()],
        )?;
        Ok(added == 1)
    }

    /// True once a prompt was recorded for this `needs_input` event.
    pub(crate) fn slack_prompt_posted(&self, event_id: i64) -> Result<bool, StoreError> {
        Ok(self
            .reader()?
            .query_row(
                "SELECT 1 FROM slack_prompts WHERE event_id = ?",
                params![event_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// True once this proof file, at this exact content digest, was posted for the task.
    pub(crate) fn slack_proof_posted(
        &self,
        task: &str,
        name: &str,
        sha256: &str,
    ) -> Result<bool, StoreError> {
        Ok(self
            .reader()?
            .query_row(
                "SELECT 1 FROM slack_proofs WHERE task = ? AND name = ? AND sha256 = ?",
                params![task, name, sha256],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Records a posted proof file; repeating the same name and digest is a no-op.
    pub(crate) fn mark_slack_proof_posted(
        &self,
        task: &str,
        name: &str,
        sha256: &str,
    ) -> Result<(), StoreError> {
        let now = self.now();
        self.writer()?.execute(
            "INSERT OR IGNORE INTO slack_proofs (task, name, sha256, posted_at) VALUES (?, ?, ?, ?)",
            params![task, name, sha256, now.as_millis()],
        )?;
        Ok(())
    }

    /// Records a posted prompt; ignored when the event already has one.
    pub(crate) fn add_slack_prompt(&self, p: &SlackPrompt) -> Result<(), StoreError> {
        let now = self.now();
        let options = serde_json::to_string(&p.options)?;
        self.writer()?.execute(
            "INSERT OR IGNORE INTO slack_prompts \
             (event_id, channel, thread_ts, message_ts, task, question, options, answer, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                p.event_id,
                p.channel,
                p.thread_ts,
                p.message_ts,
                p.task.as_str(),
                p.question,
                options,
                p.answer,
                now.as_millis()
            ],
        )?;
        Ok(())
    }

    /// The prompt shown by message `message_ts` in `channel`.
    pub(crate) fn slack_prompt_by_message(
        &self,
        channel: &str,
        message_ts: &str,
    ) -> Result<Option<SlackPrompt>, StoreError> {
        self.reader()?
            .query_row(
                &format!(
                    "SELECT {PROMPT_COLS} FROM slack_prompts WHERE channel = ? AND message_ts = ?"
                ),
                params![channel, message_ts],
                prompt_from_row,
            )
            .optional()?
            .map(prompt_of)
            .transpose()
    }

    /// The newest unanswered prompt in a thread.
    pub(crate) fn open_slack_prompt(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> Result<Option<SlackPrompt>, StoreError> {
        self.reader()?
            .query_row(
                &format!(
                    "SELECT {PROMPT_COLS} FROM slack_prompts \
                     WHERE channel = ? AND thread_ts = ? AND answer IS NULL \
                     ORDER BY event_id DESC LIMIT 1"
                ),
                params![channel, thread_ts],
                prompt_from_row,
            )
            .optional()?
            .map(prompt_of)
            .transpose()
    }

    /// Sets the answer when none is set yet; true when this call set it.
    pub(crate) fn answer_slack_prompt(
        &self,
        event_id: i64,
        answer: &str,
    ) -> Result<bool, StoreError> {
        let rows = self.writer()?.execute(
            "UPDATE slack_prompts SET answer = ? WHERE event_id = ? AND answer IS NULL",
            params![answer, event_id],
        )?;
        Ok(rows == 1)
    }
}

#[cfg(test)]
mod tests;
