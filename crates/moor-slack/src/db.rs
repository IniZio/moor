//! The front's own `SQLite` file, `slack.db`: seen envelopes, each channel's repos and the Box
//! of each thread.

use std::path::Path;
use std::time::Duration;

use moor_core::name::{BoxId, BoxName};
use rusqlite::types::Type;
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Row, Transaction, TransactionBehavior,
};

/// How long a writer waits on a locked database.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The schema version this build writes and reads. A schema change edits [`SCHEMA`] in place
/// and raises this number; there are no migrations, so a file at any other version is refused.
const SCHEMA_VERSION: i64 = 3;

/// The whole schema, created in an empty file.
const SCHEMA: &str = "
    CREATE TABLE slack_events (event_id TEXT PRIMARY KEY, seen_at INTEGER NOT NULL);
    CREATE TABLE slack_threads (
        channel TEXT NOT NULL,
        thread_ts TEXT NOT NULL,
        state TEXT NOT NULL,
        box_id INTEGER,
        box_name TEXT NOT NULL,
        repo TEXT NOT NULL,
        goal TEXT NOT NULL,
        branch TEXT,
        owner TEXT NOT NULL,
        status_ts TEXT,
        status_text TEXT,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (channel, thread_ts)
    );
    CREATE INDEX slack_threads_box ON slack_threads (box_id);
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

/// Columns of a thread row, in the order [`thread_row`] reads them.
const THREAD_COLUMNS: &str =
    "channel, thread_ts, state, box_id, box_name, repo, goal, branch, owner";

/// Where a thread's Box stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThreadState {
    /// The start was sent or is about to be; no reply has bound it yet.
    Starting,
    /// The controller named this Box.
    Bound {
        /// The Box's id.
        box_id: BoxId,
        /// The Box's branch, when the reply named one.
        branch: Option<String>,
    },
}

/// A Slack thread and the Box it drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackThread {
    /// Channel id the thread lives in.
    pub(crate) channel: String,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: String,
    /// Name of the Box the thread starts.
    pub(crate) box_name: BoxName,
    /// Repo the Box works on.
    pub(crate) repo: String,
    /// What the mention asked the Box to do.
    pub(crate) goal: String,
    /// Slack user id of the Box owner.
    pub(crate) owner: String,
    /// Where the Box stands.
    pub(crate) state: ThreadState,
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
        let sql = format!(
            "SELECT {THREAD_COLUMNS} FROM slack_threads WHERE channel = ?1 AND thread_ts = ?2"
        );
        self.conn
            .query_row(&sql, params![channel, thread_ts], thread_row)
            .optional()
    }

    /// Rows still in `starting`, oldest first.
    pub(crate) fn starting(&self) -> rusqlite::Result<Vec<SlackThread>> {
        let sql = format!(
            "SELECT {THREAD_COLUMNS} FROM slack_threads WHERE state = 'starting' \
             ORDER BY created_at, channel, thread_ts"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], thread_row)?;
        rows.collect()
    }

    /// Records `thread` in `starting` at `now`; a thread that already has a row is an error.
    pub(crate) fn insert_starting(&self, thread: &SlackThread, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO slack_threads \
             (channel, thread_ts, state, box_name, repo, goal, owner, created_at) \
             VALUES (?1, ?2, 'starting', ?3, ?4, ?5, ?6, ?7)",
            params![
                thread.channel,
                thread.thread_ts,
                thread.box_name.as_str(),
                thread.repo,
                thread.goal,
                thread.owner,
                now
            ],
        )?;
        Ok(())
    }

    /// Binds a `starting` row to the Box the controller named.
    pub(crate) fn bind(
        &self,
        channel: &str,
        thread_ts: &str,
        box_id: BoxId,
        branch: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE slack_threads SET state = 'bound', box_id = ?3, branch = ?4 \
             WHERE channel = ?1 AND thread_ts = ?2 AND state = 'starting'",
            params![channel, thread_ts, box_id.get(), branch],
        )?;
        Ok(())
    }

    /// Deletes a row still in `starting`, so the thread can start again.
    pub(crate) fn forget_starting(&self, channel: &str, thread_ts: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM slack_threads WHERE channel = ?1 AND thread_ts = ?2 AND state = 'starting'",
            params![channel, thread_ts],
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

/// Reads one thread row selected with [`THREAD_COLUMNS`].
fn thread_row(r: &Row<'_>) -> rusqlite::Result<SlackThread> {
    let bad = |col: usize, why: String| {
        rusqlite::Error::FromSqlConversionFailure(col, Type::Text, why.into())
    };
    let state: String = r.get(2)?;
    let state = match (state.as_str(), r.get::<_, Option<i64>>(3)?) {
        ("starting", _) => ThreadState::Starting,
        ("bound", Some(id)) => ThreadState::Bound {
            box_id: BoxId::new(id),
            branch: r.get(7)?,
        },
        (other, _) => return Err(bad(2, format!("thread state `{other}`"))),
    };
    let name: String = r.get(4)?;
    Ok(SlackThread {
        channel: r.get(0)?,
        thread_ts: r.get(1)?,
        box_name: BoxName::parse_allow_bare(&name).map_err(|e| bad(4, e.to_string()))?,
        repo: r.get(5)?,
        goal: r.get(6)?,
        owner: r.get(8)?,
        state,
    })
}

mod channel_repos;
pub(crate) use channel_repos::ChannelRepo;

#[cfg(test)]
mod tests;
