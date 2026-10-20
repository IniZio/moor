//! The front's own `SQLite` file, `slack.db`: seen envelopes and the Box of each thread.

use std::path::Path;
use std::time::Duration;

use moor_core::name::{BoxId, BoxName};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

/// How long a writer waits on a locked database.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Ordered, additive migrations; entry `n` moves `user_version` from `n` to `n + 1`.
const MIGRATIONS: &[&str] = &["
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
"];

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
    /// Opens (creating if needed) the database at `path` and applies missing migrations.
    ///
    /// # Errors
    /// Returns the `SQLite` error text, or says so when the file was written by a newer build.
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
        migrate(&mut conn).map_err(|e| e.to_string())?;
        let known = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
        let found: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if found > known {
            return Err(format!(
                "slack.db schema {found} is newer than this build knows ({known})"
            ));
        }
        Ok(Self { conn })
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

/// Applies the migrations not yet recorded in `user_version`, in one write transaction so
/// two connections that open at once cannot both apply them.
fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (version, sql) in (1_i64..).zip(MIGRATIONS).skip_while(|(v, _)| *v <= found) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
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

#[cfg(test)]
mod tests;
