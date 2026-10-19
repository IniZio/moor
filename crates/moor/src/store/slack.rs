//! Slack event dedup and the binding of a Slack thread to a Box's primary Session.

use moor_core::BoxName;
use rusqlite::{params, OptionalExtension};

use super::bindings::insert_binding;
use super::{parsed, Store, StoreError};

/// DDL for the Slack tables; idempotent so existing version-1 files gain them on open.
pub(super) const SLACK_SQL: &str = include_str!("slack.sql");

/// A Slack thread bound to a Box's Session, with the user who started it and its status message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackThread {
    /// Channel id the thread lives in.
    pub(crate) channel: String,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: String,
    /// The Box whose Session the thread drives.
    pub(crate) task: BoxName,
    /// Slack user id of the Box owner.
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
    /// The Box whose Session is waiting.
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
    "p.event_id, p.channel, p.thread_ts, p.message_ts, b.name, p.question, p.options, p.answer";

/// Joins a prompt to the Box its Session belongs to.
const PROMPT_FROM: &str =
    "slack_prompts p JOIN sessions s ON s.id = p.session LEFT JOIN boxes b ON b.id = s.box";

/// Front name under which Slack threads are bound.
const FRONT: &str = "slack";

/// Conversation key of a Slack thread within the `bindings` table.
fn conv_key(channel: &str, thread_ts: &str) -> String {
    format!("{channel}/{thread_ts}")
}

/// Splits a conversation key back into channel and thread timestamp.
fn split_conv_key(key: &str) -> Result<(String, String), StoreError> {
    key.split_once('/')
        .map(|(c, t)| (c.to_owned(), t.to_owned()))
        .ok_or_else(|| StoreError::Corrupt(format!("slack conversation key `{key}`")))
}

/// The newest open Box with this name and its first Session.
fn open_box_session(conn: &rusqlite::Connection, name: &str) -> Result<(i64, i64), StoreError> {
    conn.query_row(
        "SELECT b.id, s.id FROM boxes b JOIN sessions s ON s.box = b.id AND s.ordinal = 1 \
         WHERE b.name = ? AND b.state NOT IN ('closed','rejected') ORDER BY b.id DESC LIMIT 1",
        params![name],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?
    .ok_or_else(|| StoreError::Invalid(format!("no open Box named `{name}`")))
}

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
        task: parsed("slack prompt task", &task, BoxName::parse_allow_bare)?,
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

    /// The Task bound to a Slack thread; `None` when unbound or bound to a host Session.
    pub(crate) fn slack_thread(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> Result<Option<BoxName>, StoreError> {
        Ok(self
            .binding(FRONT, &conv_key(channel, thread_ts))?
            .and_then(|b| b.task))
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
                "SELECT owner FROM bindings WHERE front = ? AND conv_key = ?",
                params![FRONT, conv_key(channel, thread_ts)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// All Box-bound threads, oldest first; host-bound threads are skipped.
    pub(crate) fn slack_threads(&self) -> Result<Vec<SlackThread>, StoreError> {
        let mut threads = Vec::new();
        for b in self.bindings(FRONT)? {
            let Some(task) = b.task else { continue };
            let (channel, thread_ts) = split_conv_key(&b.key)?;
            threads.push(SlackThread {
                channel,
                thread_ts,
                task,
                owner: b.owner,
                status_ts: b.status_ts,
            });
        }
        Ok(threads)
    }

    /// Records the timestamp of a thread's status message.
    pub(crate) fn set_slack_status_ts(
        &self,
        channel: &str,
        thread_ts: &str,
        ts: &str,
    ) -> Result<(), StoreError> {
        self.writer()?.execute(
            "UPDATE bindings SET status_ts = ? WHERE front = ? AND conv_key = ?",
            params![ts, FRONT, conv_key(channel, thread_ts)],
        )?;
        Ok(())
    }

    /// Binds a thread to the primary Session of the newest open Box named `task`, owned by `owner`.
    /// The first binding wins; true when this call created it. Errors when no such Box is open.
    pub(crate) fn bind_slack_thread(
        &self,
        channel: &str,
        thread_ts: &str,
        task: &BoxName,
        owner: &str,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let writer = self.writer()?;
        let (_, session) = open_box_session(&writer, task.as_str())?;
        let key = conv_key(channel, thread_ts);
        let added = insert_binding(&writer, FRONT, &key, session, owner, now.as_millis())?;
        drop(writer);
        Ok(added)
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

    /// Records a posted prompt; ignored when the event already has one.
    pub(crate) fn add_slack_prompt(&self, p: &SlackPrompt) -> Result<(), StoreError> {
        let now = self.now();
        let options = serde_json::to_string(&p.options)?;
        let writer = self.writer()?;
        let (_, session) = open_box_session(&writer, p.task.as_str())?;
        writer.execute(
            "INSERT OR IGNORE INTO slack_prompts \
             (event_id, channel, thread_ts, message_ts, session, question, options, answer, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                p.event_id,
                p.channel,
                p.thread_ts,
                p.message_ts,
                session,
                p.question,
                options,
                p.answer,
                now.as_millis()
            ],
        )?;
        drop(writer);
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
                    "SELECT {PROMPT_COLS} FROM {PROMPT_FROM} WHERE b.id IS NOT NULL AND p.channel = ? AND p.message_ts = ?"
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
                    "SELECT {PROMPT_COLS} FROM {PROMPT_FROM} \
                     WHERE b.id IS NOT NULL AND p.channel = ? AND p.thread_ts = ? AND p.answer IS NULL \
                     ORDER BY p.event_id DESC LIMIT 1"
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
