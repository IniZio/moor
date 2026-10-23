//! Each thread's inbox: what its starter wrote, recorded before Slack gets the ack, each item
//! tied to the Slack user who wrote it.

use rusqlite::params;

use super::{Db, SlackBox};
use crate::command::{report, BoxEvent};

/// What an inbox item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemKind {
    /// A message for the coordinator.
    Message,
}

impl ItemKind {
    /// The `kind` column's value.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
        }
    }
}

/// One item a thread's inbox holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InboxItem {
    /// Channel id of the thread.
    pub(crate) channel: String,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: String,
    /// What the item is.
    pub(crate) kind: ItemKind,
    /// Slack user id of the person who wrote it.
    pub(crate) author: String,
    /// The text, without leading mentions.
    pub(crate) text: String,
    /// Timestamp of the Slack message it came from.
    pub(crate) message_ts: Option<String>,
    /// The Slack event id it came from, so a redelivery is recorded once.
    pub(crate) source: String,
}

impl Db {
    /// Records `item` at `now`; an item from a source already recorded is an error.
    pub(crate) fn queue(&self, item: &InboxItem, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO slack_inbox (channel, thread_ts, kind, author, text, source, created_at, message_ts) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                item.channel,
                item.thread_ts,
                item.kind.as_str(),
                item.author,
                item.text,
                item.source,
                now,
                item.message_ts
            ],
        )?;
        Ok(())
    }

    /// Queues the notice of `event` on `boxed` for the next turn of its thread, once per
    /// Box, event and `cursor`; the cursor tells apart events that repeat, such as each
    /// finished turn. Returns whether this call wrote it.
    pub(crate) fn notice(
        &self,
        boxed: &SlackBox,
        event: BoxEvent,
        cursor: Option<i64>,
    ) -> rusqlite::Result<bool> {
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO slack_inbox \
             (channel, thread_ts, kind, text, source, created_at) \
             VALUES (?1, ?2, 'notice', ?3, ?4, unixepoch())",
            params![
                boxed.channel,
                boxed.thread_ts,
                event.notice(&boxed.box_name),
                notice_source(boxed, event, cursor)
            ],
        )?;
        Ok(added == 1)
    }

    /// Queues the report of the finished turn of `boxed` at `cursor`, which starts a turn of
    /// its thread on its own; `said` is the agent's last message. Once per Box and cursor, so
    /// a refold, a restart or a redelivered event queues it once. Returns whether this call
    /// wrote it.
    pub(crate) fn report(
        &self,
        boxed: &SlackBox,
        cursor: i64,
        said: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO slack_inbox \
             (channel, thread_ts, kind, text, source, created_at) \
             VALUES (?1, ?2, 'report', ?3, ?4, unixepoch())",
            params![
                boxed.channel,
                boxed.thread_ts,
                report(&boxed.box_name, said),
                notice_source(boxed, BoxEvent::Finished, Some(cursor))
            ],
        )?;
        Ok(added == 1)
    }

    /// Whether the notice of `event` on `boxed` at `cursor` was queued.
    pub(crate) fn has_notice(
        &self,
        boxed: &SlackBox,
        event: BoxEvent,
        cursor: Option<i64>,
    ) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM slack_inbox WHERE source = ?1)",
            params![notice_source(boxed, event, cursor)],
            |r| r.get(0),
        )
    }

    /// The items of a thread, in the order they were recorded.
    #[cfg(test)]
    pub(crate) fn inbox(&self, channel: &str, thread_ts: &str) -> rusqlite::Result<Vec<InboxItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel, thread_ts, author, text, source FROM slack_inbox \
             WHERE channel = ?1 AND thread_ts = ?2 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![channel, thread_ts], |r| {
            Ok(InboxItem {
                channel: r.get(0)?,
                thread_ts: r.get(1)?,
                kind: ItemKind::Message,
                author: r.get(2)?,
                text: r.get(3)?,
                message_ts: None,
                source: r.get(4)?,
            })
        })?;
        rows.collect()
    }
}

/// The `source` of the notice of `event` on `boxed` at `cursor`. A Box name is never reused,
/// so the source names one event of one Box.
fn notice_source(boxed: &SlackBox, event: BoxEvent, cursor: Option<i64>) -> String {
    let name = boxed.box_name.as_str();
    let key = event.key();
    cursor.map_or_else(
        || format!("box:{name}:{key}"),
        |c| format!("box:{name}:{key}:{c}"),
    )
}
