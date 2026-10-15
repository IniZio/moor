//! Slack event dedup and the thread-to-Task binding.

use moor_core::TaskName;
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
    pub(crate) task: TaskName,
    /// Slack user id of the Task owner.
    pub(crate) owner: String,
    /// Timestamp of the edited status message, once posted.
    pub(crate) status_ts: Option<String>,
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
    ) -> Result<Option<TaskName>, StoreError> {
        let raw: Option<String> = self
            .reader()?
            .query_row(
                "SELECT task FROM slack_threads WHERE channel = ? AND thread_ts = ?",
                params![channel, thread_ts],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|name| parsed("slack thread task", &name, TaskName::parse))
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
                    task: parsed("slack thread task", &task, TaskName::parse)?,
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
        task: &TaskName,
        owner: &str,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let added = self.writer()?.execute(
            "INSERT OR IGNORE INTO slack_threads (channel, thread_ts, task, owner, created_at) VALUES (?, ?, ?, ?, ?)",
            params![channel, thread_ts, task.as_str(), owner, now.as_millis()],
        )?;
        Ok(added == 1)
    }
}

#[cfg(test)]
mod tests;
