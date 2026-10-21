//! The front's own `SQLite` file, `slack.db`: seen envelopes, each channel's repos, each
//! thread with the Boxes it asked for, what its starter wrote, its coordinator turns and what
//! those turns did to its Boxes.

use std::path::Path;
use std::time::Duration;

use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};

/// How long a writer waits on a locked database.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The schema version this build writes and reads. A schema change edits [`SCHEMA`] in place
/// and raises this number; there are no migrations, so a file at any other version is refused.
const SCHEMA_VERSION: i64 = 10;

/// The whole schema, created in an empty file.
const SCHEMA: &str = "
    CREATE TABLE slack_events (event_id TEXT PRIMARY KEY, seen_at INTEGER NOT NULL);
    CREATE TABLE slack_threads (
        channel TEXT NOT NULL,
        thread_ts TEXT NOT NULL,
        owner TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        workdir TEXT,
        root_session INTEGER,
        last_turn_at INTEGER,
        PRIMARY KEY (channel, thread_ts)
    );
    CREATE TABLE slack_boxes (
        box_name TEXT PRIMARY KEY,
        channel TEXT NOT NULL,
        thread_ts TEXT NOT NULL,
        n INTEGER NOT NULL,
        repo TEXT NOT NULL,
        goal TEXT NOT NULL,
        parent INTEGER,
        requested_by TEXT NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('starting', 'bound', 'closing', 'gone')),
        box_id INTEGER,
        branch TEXT,
        status_ts TEXT,
        status_text TEXT,
        created_at INTEGER NOT NULL,
        UNIQUE (channel, thread_ts, n)
    );
    CREATE INDEX slack_boxes_box ON slack_boxes (box_id);
    CREATE TABLE slack_inbox (
        id INTEGER PRIMARY KEY,
        channel TEXT NOT NULL,
        thread_ts TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('message', 'notice')),
        author TEXT CHECK (kind = 'notice' OR author IS NOT NULL),
        text TEXT NOT NULL,
        source TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        turn_id INTEGER,
        done_at INTEGER
    );
    CREATE INDEX slack_inbox_thread ON slack_inbox (channel, thread_ts, id);
    CREATE TABLE slack_turns (
        id INTEGER PRIMARY KEY,
        channel TEXT NOT NULL,
        thread_ts TEXT NOT NULL,
        paid_by TEXT NOT NULL,
        rerun_of INTEGER,
        started_at INTEGER NOT NULL,
        pid INTEGER,
        pid_started INTEGER,
        placeholder_ts TEXT,
        ended_at INTEGER,
        end TEXT CHECK (end IN
            ('replied', 'failed', 'killed', 'unsafe', 'lost', 'no_token', 'token_refused')),
        reply_text TEXT,
        posted_at INTEGER,
        context_bytes INTEGER,
        cost_usd REAL
    );
    CREATE INDEX slack_turns_thread ON slack_turns (channel, thread_ts, id);
    CREATE TABLE slack_actions (
        turn_id INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        tool TEXT NOT NULL,
        box_name TEXT NOT NULL,
        requested_by TEXT NOT NULL,
        args TEXT NOT NULL,
        outcome TEXT,
        created_at INTEGER NOT NULL,
        done_at INTEGER,
        PRIMARY KEY (turn_id, seq)
    );
    CREATE TABLE slack_channel_repos (
        channel TEXT NOT NULL,
        repo TEXT NOT NULL,
        added_by TEXT NOT NULL,
        added_at INTEGER NOT NULL,
        is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
        PRIMARY KEY (channel, repo)
    );
    CREATE UNIQUE INDEX slack_channel_repos_one_default
        ON slack_channel_repos (channel) WHERE is_default = 1;
";

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
    /// Opens the database at `path`, creating the schema when the file is new or empty.
    ///
    /// # Errors
    /// Returns the `SQLite` error text, or says to start with a fresh `slack.db` when the file
    /// is at another schema version. A refused file is left exactly as it was found.
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let refuse = |found: i64| {
            format!(
                "slack.db is schema {found} and this build needs {SCHEMA_VERSION}. \
                 Start with a fresh slack.db."
            )
        };
        if path.exists() {
            let (found, tables) = peek(path).map_err(|e| e.to_string())?;
            if found != SCHEMA_VERSION && (found, tables) != (0, 0) {
                return Err(refuse(found));
            }
        }
        let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(|e| e.to_string())?;
        create_if_empty(&mut conn).map_err(|e| e.to_string())?;
        let found = version(&conn).map_err(|e| e.to_string())?;
        if found != SCHEMA_VERSION {
            return Err(refuse(found));
        }
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

/// The file's `user_version`.
fn version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// The file's `user_version` and table count, read through a read-only connection. Closing a
/// read-write connection checkpoints a leftover `-wal` file into the database, which would
/// change a file that is then refused.
fn peek(path: &Path) -> rusqlite::Result<(i64, i64)> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let tables = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    Ok((version(&conn)?, tables))
}

/// Creates the schema in a file that has no version and no tables. The check runs inside a
/// write transaction, so two connections that open a new file at once create the schema once.
fn create_if_empty(conn: &mut Connection) -> rusqlite::Result<()> {
    if version(conn)? != 0 {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let tables: i64 = tx.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    if version(&tx)? == 0 && tables == 0 {
        tx.execute_batch(SCHEMA)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    tx.commit()
}

mod actions;

mod boxes;
pub(crate) use boxes::{BoxState, FedBox, SlackBox};

mod channel_repos;
pub(crate) use channel_repos::ChannelRepo;

mod host;

mod inbox;
pub(crate) use inbox::{InboxItem, ItemKind};

mod turns;
pub(crate) use turns::{End, OpenTurn, TurnRow};

#[cfg(test)]
mod tests;
