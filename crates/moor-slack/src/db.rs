//! The front's own `SQLite` file, `slack.db`: seen envelopes, each channel's repos, each
//! thread with the Boxes it asked for, what its starter wrote, its coordinator turns, what
//! those turns did to its Boxes, the questions its Boxes asked and the artifact versions
//! shown in their threads.

use std::path::Path;
use std::time::Duration;

use moor_migrate::{Legacy, Migration};
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};

/// How long a writer waits on a locked database.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The pre-migration `user_version` that migration 1 reproduces; a file at it is adopted.
const BASELINE_VERSION: i64 = 16;

/// slack.db's migrations, oldest first. Never edit an applied one; add a new file.
fn schema() -> Result<Vec<Migration>, moor_migrate::Error> {
    moor_migrate::migrations(&[
        (
            "V1__baseline.sql",
            include_str!("../migrations/V1__baseline.sql"),
        ),
        (
            "V2__box_prompt.sql",
            include_str!("../migrations/V2__box_prompt.sql"),
        ),
        (
            "V3__user_emails.sql",
            include_str!("../migrations/V3__user_emails.sql"),
        ),
    ])
}

/// A Slack thread the front answers in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackThread {
    /// Channel id the thread lives in.
    pub(crate) channel: String,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: String,
    /// Slack user id of the person who started the thread.
    pub(crate) owner: String,
}

/// The front's database, separate from the controller's.
pub(crate) struct Db {
    /// The single connection.
    conn: Connection,
}

impl Db {
    /// Opens the database at `path` and brings it to the latest migration; one process
    /// migrates while others wait.
    ///
    /// # Errors
    /// Returns the `SQLite` error text, says to start with a fresh `slack.db` when the file
    /// predates migrations at a version this build cannot adopt, or refuses a file a newer build
    /// wrote. A refused file is left exactly as it was found.
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let set = schema().map_err(|e| e.to_string())?;
        if path.exists() {
            let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
            conn.busy_timeout(BUSY_TIMEOUT).map_err(|e| e.to_string())?;
            moor_migrate::check(&conn, &set, legacy).map_err(refused)?;
        }
        let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(|e| e.to_string())?;
        moor_migrate::migrate(&mut conn, &set, legacy).map_err(refused)?;
        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(|e| e.to_string())?;
        Ok(Self { conn })
    }

    /// Runs `write` in one immediate transaction, so nothing it wrote stays when it fails.
    pub(crate) fn atomically<T>(
        &self,
        write: impl FnOnce() -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let out = write()?;
        tx.commit()?;
        Ok(out)
    }

    /// Records a Slack event id at `now`; true the first time, false on redelivery.
    pub(crate) fn event_once(&self, event_id: &str, now: i64) -> rusqlite::Result<bool> {
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO slack_events (event_id, seen_at) VALUES (?1, ?2)",
            params![event_id, now],
        )?;
        Ok(added == 1)
    }

    /// The thread row for `(channel, thread_ts)`, if any.
    pub(crate) fn thread(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> rusqlite::Result<Option<SlackThread>> {
        self.conn
            .query_row(
                "SELECT channel, thread_ts, owner FROM slack_threads \
                 WHERE channel = ?1 AND thread_ts = ?2",
                params![channel, thread_ts],
                |r| {
                    Ok(SlackThread {
                        channel: r.get(0)?,
                        thread_ts: r.get(1)?,
                        owner: r.get(2)?,
                    })
                },
            )
            .optional()
    }

    /// The Slack users who own a thread, each once.
    pub(crate) fn thread_owners(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT owner FROM slack_threads ORDER BY owner")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    }

    /// Records `thread` at `now`; a thread that already has a row is an error.
    pub(crate) fn insert_thread(&self, thread: &SlackThread, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO slack_threads (channel, thread_ts, owner, created_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![thread.channel, thread.thread_ts, thread.owner, now],
        )?;
        Ok(())
    }
}

/// What a file without a migration history holds, judged by its `user_version`. The check
/// runs on a read-only connection first: closing a read-write one checkpoints a leftover `-wal`
/// file into the database, which would change a file that is then refused.
fn legacy(conn: &Connection) -> rusqlite::Result<Legacy> {
    let found: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    Ok(match (found, tables) {
        (0, 0) => Legacy::Empty,
        (BASELINE_VERSION, _) => Legacy::Baseline,
        _ => Legacy::Other(found.to_string()),
    })
}

/// The message for a file this build will not migrate.
fn refused(e: moor_migrate::Error) -> String {
    match e {
        moor_migrate::Error::Unadoptable(found) => format!(
            "slack.db is schema {found} and this build needs {BASELINE_VERSION}. \
             Start with a fresh slack.db."
        ),
        other => format!("slack.db cannot be migrated by this build: {other}"),
    }
}

mod actions;

mod artifacts;
pub(crate) use artifacts::ArtifactShown;

mod boxes;
pub(crate) use boxes::{BoxState, FedBox, SlackBox};

mod channel_repos;
pub(crate) use channel_repos::ChannelRepo;

mod emails;

mod host;

mod inbox;
pub(crate) use inbox::{InboxItem, ItemKind};

mod prompts;
pub(crate) use prompts::{Posted, Prompt, PromptState};

mod turns;
pub(crate) use turns::{End, OpenTurn, TurnRow};

#[cfg(test)]
mod tests;
