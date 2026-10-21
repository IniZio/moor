//! Each thread's inbox: what its starter wrote, recorded before Slack gets the ack, each item
//! tied to the Slack user who wrote it.

use rusqlite::params;

use super::Db;

/// What an inbox item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemKind {
    /// A message for the coordinator.
    Message,
    /// A `close`, which the front handles itself.
    Close,
}

impl ItemKind {
    /// The `kind` column's value.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Close => "close",
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
    /// The Slack event id it came from, so a redelivery is recorded once.
    pub(crate) source: String,
}

impl Db {
    /// Records `item` at `now`; an item from a source already recorded is an error.
    pub(crate) fn queue(&self, item: &InboxItem, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO slack_inbox (channel, thread_ts, kind, author, text, source, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                item.channel,
                item.thread_ts,
                item.kind.as_str(),
                item.author,
                item.text,
                item.source,
                now
            ],
        )?;
        Ok(())
    }

    /// The items of a thread, in the order they were recorded.
    #[cfg(test)]
    pub(crate) fn inbox(&self, channel: &str, thread_ts: &str) -> rusqlite::Result<Vec<InboxItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel, thread_ts, kind, author, text, source FROM slack_inbox \
             WHERE channel = ?1 AND thread_ts = ?2 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![channel, thread_ts], |r| {
            let kind: String = r.get(2)?;
            Ok(InboxItem {
                channel: r.get(0)?,
                thread_ts: r.get(1)?,
                kind: if kind == "close" {
                    ItemKind::Close
                } else {
                    ItemKind::Message
                },
                author: r.get(3)?,
                text: r.get(4)?,
                source: r.get(5)?,
            })
        })?;
        rows.collect()
    }
}
