//! Each thread's coordinator turns: a turn row is written, and the inbox items it takes are
//! marked, before its process starts. Its closing line is stored before it is posted.

use rusqlite::{params, OptionalExtension as _};

use super::Db;
use crate::coordinator::context::{Said, Speaker};

/// How a turn ended, as the `end` column stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// The coordinator replied.
    Replied,
    /// The coordinator could not run, or stopped with an error.
    Failed,
    /// The turn ran past its limit.
    Killed,
    /// The coordinator loaded more than the front gave it, or its workdir was unsafe.
    Unsafe,
    /// The starter has linked no credential.
    NoToken,
    /// The provider refused the starter's credential.
    TokenRefused,
    /// The front restarted while the turn ran.
    Lost,
}

impl End {
    /// The `end` column's value.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Replied => "replied",
            Self::Failed => "failed",
            Self::Killed => "killed",
            Self::Unsafe => "unsafe",
            Self::NoToken => "no_token",
            Self::TokenRefused => "token_refused",
            Self::Lost => "lost",
        }
    }
}

/// A turn taken by the scheduler, with what it needs from its thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnRow {
    /// The turn's id.
    pub(crate) id: i64,
    /// Channel id of the thread.
    pub(crate) channel: String,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: String,
    /// Slack user id of the thread's starter, who pays for the turn.
    pub(crate) owner: String,
    /// The turn a restart cut off that this turn runs again.
    pub(crate) rerun_of: Option<i64>,
}

/// A turn the front left without an end when it stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unended {
    /// The turn.
    pub(crate) row: TurnRow,
    /// When it started, in unix seconds.
    pub(crate) started_at: i64,
    /// Its process id and that process's start time in clock ticks after boot, when both
    /// were recorded.
    pub(crate) process: Option<(u32, i64)>,
}

/// The turn a thread runs now, as its tools see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenTurn {
    /// The turn's id.
    pub(crate) id: i64,
    /// The turn a restart cut off that this turn runs again.
    pub(crate) rerun_of: Option<i64>,
    /// Slack user ids of the people whose messages the turn holds, each once.
    pub(crate) authors: Vec<String>,
}

/// The columns [`turn_row`] reads.
const TURN_COLUMNS: &str = "u.id, u.channel, u.thread_ts, u.paid_by, u.rerun_of";

/// A [`TurnRow`] from [`TURN_COLUMNS`].
fn turn_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TurnRow> {
    Ok(TurnRow {
        id: r.get(0)?,
        channel: r.get(1)?,
        thread_ts: r.get(2)?,
        owner: r.get(3)?,
        rerun_of: r.get(4)?,
    })
}

impl Db {
    /// Takes the next turn at `now`: the oldest thread with a waiting message or report and
    /// no open turn gets a turn row, and every waiting item of that thread is marked as taken
    /// by it. `None` when no thread waits.
    pub(crate) fn take_turn(&self, now: i64) -> rusqlite::Result<Option<TurnRow>> {
        self.atomically(|| {
            let waiting = self
                .conn
                .query_row(
                    "SELECT i.channel, i.thread_ts, t.owner FROM slack_inbox i \
                     JOIN slack_threads t ON t.channel = i.channel AND t.thread_ts = i.thread_ts \
                     WHERE i.kind IN ('message', 'report') AND i.turn_id IS NULL AND i.done_at IS NULL \
                     AND NOT EXISTS (SELECT 1 FROM slack_turns u WHERE u.channel = i.channel \
                         AND u.thread_ts = i.thread_ts AND u.ended_at IS NULL) \
                     ORDER BY i.id LIMIT 1",
                    [],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((channel, thread_ts, owner)) = waiting else {
                return Ok(None);
            };
            self.conn.execute(
                "INSERT INTO slack_turns (channel, thread_ts, paid_by, started_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![channel, thread_ts, owner, now],
            )?;
            let id = self.conn.last_insert_rowid();
            self.conn.execute(
                "UPDATE slack_inbox SET turn_id = ?1 WHERE channel = ?2 AND thread_ts = ?3 \
                 AND kind IN ('message', 'notice', 'report') AND turn_id IS NULL AND done_at IS NULL",
                params![id, channel, thread_ts],
            )?;
            Ok(Some(TurnRow {
                id,
                channel,
                thread_ts,
                owner,
                rerun_of: None,
            }))
        })
    }

    /// Records the process id of turn `id` and when that process started, in clock ticks
    /// after boot, when that could be read.
    pub(crate) fn set_pid(&self, id: i64, pid: u32, started: Option<i64>) -> rusqlite::Result<()> {
        let sql = "UPDATE slack_turns SET pid = ?2, pid_started = ?3 WHERE id = ?1";
        self.conn.execute(sql, params![id, pid, started]).map(drop)
    }

    /// Stores how turn `id` ended and the line to post, with its context size and reported
    /// cost for the log, before the line is posted.
    pub(crate) fn store_reply(
        &self,
        id: i64,
        end: End,
        text: &str,
        context_bytes: Option<usize>,
        cost_usd: Option<f64>,
    ) -> rusqlite::Result<()> {
        let bytes = context_bytes.and_then(|b| i64::try_from(b).ok());
        self.conn
            .execute(
                "UPDATE slack_turns SET end = ?2, reply_text = ?3, context_bytes = ?4, \
                 cost_usd = ?5 WHERE id = ?1",
                params![id, end.as_str(), text, bytes, cost_usd],
            )
            .map(drop)
    }

    /// Marks turn `id`'s line posted at `now`, the turn ended and its items done.
    pub(crate) fn mark_posted(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        self.atomically(|| {
            self.conn.execute(
                "UPDATE slack_turns SET posted_at = ?2, ended_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            self.conn.execute(
                "UPDATE slack_inbox SET done_at = ?2 WHERE turn_id = ?1",
                params![id, now],
            )?;
            self.conn.execute(
                "UPDATE slack_threads SET last_turn_at = ?2 WHERE (channel, thread_ts) = \
                 (SELECT channel, thread_ts FROM slack_turns WHERE id = ?1)",
                params![id, now],
            )?;
            Ok(())
        })
    }

    /// Turns whose line was stored and not yet marked posted, with that line.
    pub(crate) fn unposted(&self) -> rusqlite::Result<Vec<(TurnRow, String)>> {
        let sql = format!(
            "SELECT {TURN_COLUMNS}, u.reply_text FROM slack_turns u WHERE u.reply_text IS NOT NULL \
             AND u.posted_at IS NULL ORDER BY u.id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| Ok((turn_row(r)?, r.get(5)?)))?;
        rows.collect()
    }

    /// Turns with no end and no stored line.
    pub(crate) fn unended(&self) -> rusqlite::Result<Vec<Unended>> {
        let sql = format!(
            "SELECT {TURN_COLUMNS}, u.started_at, u.pid, u.pid_started FROM slack_turns u \
             WHERE u.ended_at IS NULL \
             AND u.reply_text IS NULL ORDER BY u.id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok(Unended {
                row: turn_row(r)?,
                started_at: r.get(5)?,
                process: r.get::<_, Option<u32>>(6)?.zip(r.get(7)?),
            })
        })?;
        rows.collect()
    }

    /// Marks turn `lost` lost at `now` and records a turn that runs it again, with its items.
    /// Returns the new turn.
    pub(crate) fn rerun(&self, lost: &TurnRow, now: i64) -> rusqlite::Result<TurnRow> {
        self.atomically(|| {
            self.conn.execute(
                "UPDATE slack_turns SET end = 'lost', ended_at = ?2 WHERE id = ?1",
                params![lost.id, now],
            )?;
            self.conn.execute(
                "INSERT INTO slack_turns \
                 (channel, thread_ts, paid_by, rerun_of, started_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![lost.channel, lost.thread_ts, lost.owner, lost.id, now],
            )?;
            let id = self.conn.last_insert_rowid();
            self.conn.execute(
                "UPDATE slack_inbox SET turn_id = ?2 WHERE turn_id = ?1",
                params![lost.id, id],
            )?;
            Ok(TurnRow {
                id,
                rerun_of: Some(lost.id),
                ..lost.clone()
            })
        })
    }

    /// The thread's turn with no end, with the authors of the messages it holds; `None` when
    /// no turn runs.
    pub(crate) fn open_turn(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> rusqlite::Result<Option<OpenTurn>> {
        let turn = self
            .conn
            .query_row(
                "SELECT id, rerun_of FROM slack_turns WHERE channel = ?1 AND thread_ts = ?2 \
                 AND ended_at IS NULL ORDER BY id DESC LIMIT 1",
                params![channel, thread_ts],
                |r| Ok((r.get::<_, i64>(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((id, rerun_of)) = turn else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT author FROM slack_inbox WHERE turn_id = ?1 AND kind = 'message' \
             ORDER BY author",
        )?;
        let authors = stmt
            .query_map(params![id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(OpenTurn {
            id,
            rerun_of,
            authors,
        }))
    }

    /// The id and text of the message that opened the thread, when it was recorded.
    pub(crate) fn first_request(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> rusqlite::Result<Option<(i64, String)>> {
        self.conn
            .query_row(
                "SELECT id, text FROM slack_inbox WHERE channel = ?1 AND thread_ts = ?2 \
                 AND kind = 'message' ORDER BY id LIMIT 1",
                params![channel, thread_ts],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    /// The items turn `id` takes, in the order they were recorded.
    pub(crate) fn turn_items(&self, id: i64) -> rusqlite::Result<Vec<(i64, Said)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, author, text FROM slack_inbox WHERE turn_id = ?1 \
             AND kind IN ('message', 'notice', 'report') ORDER BY id",
        )?;
        let rows = stmt.query_map(params![id], |r| Ok((r.get(0)?, said(r.get(1)?, r.get(2)?))))?;
        rows.collect()
    }

    /// What the thread said before turn `id`, in thread order: each earlier turn's items,
    /// then its reply when the coordinator replied.
    pub(crate) fn history(
        &self,
        channel: &str,
        thread_ts: &str,
        id: i64,
    ) -> rusqlite::Result<Vec<(i64, Said)>> {
        let mut stmt = self.conn.prepare(
            "SELECT turn_id, 0, id, author, text FROM slack_inbox \
             WHERE channel = ?1 AND thread_ts = ?2 AND turn_id < ?3 \
             AND kind IN ('message', 'notice', 'report') \
             UNION ALL SELECT id, 1, 0, NULL, reply_text FROM slack_turns \
             WHERE channel = ?1 AND thread_ts = ?2 AND id < ?3 AND end = 'replied' \
             ORDER BY 1, 2, 3",
        )?;
        let rows = stmt.query_map(params![channel, thread_ts, id], |r| {
            let reply: i64 = r.get(1)?;
            let text: String = r.get(4)?;
            Ok(if reply == 1 {
                (
                    0,
                    Said {
                        speaker: Speaker::Coordinator,
                        text,
                    },
                )
            } else {
                (r.get(2)?, said(r.get(3)?, text))
            })
        })?;
        rows.collect()
    }
}

/// An inbox item as said: a person's message, or a front notice when it has no author.
fn said(author: Option<String>, text: String) -> Said {
    let speaker = author.map_or(Speaker::Front, Speaker::Person);
    Said { speaker, text }
}

#[cfg(test)]
mod tests;
