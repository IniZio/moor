//! Slack event dedup and the thread-to-Task binding.

use moor_core::TaskName;
use rusqlite::{params, OptionalExtension};

use super::{parsed, Store, StoreError};

/// DDL for the Slack tables; idempotent so existing version-1 files gain them on open.
pub(super) const SLACK_SQL: &str = include_str!("slack.sql");

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

    /// Binds a thread to a Task; the first binding wins. True when this call created it.
    pub(crate) fn bind_slack_thread(
        &self,
        channel: &str,
        thread_ts: &str,
        task: &TaskName,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let added = self.writer()?.execute(
            "INSERT OR IGNORE INTO slack_threads (channel, thread_ts, task, created_at) VALUES (?, ?, ?, ?)",
            params![channel, thread_ts, task.as_str(), now.as_millis()],
        )?;
        Ok(added == 1)
    }
}

#[cfg(test)]
mod tests;
