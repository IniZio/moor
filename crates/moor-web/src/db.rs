//! The front's own `SQLite` state: request keys and the Watch cursor.

use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::name::SessionId;
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// How long a writer waits on a locked database.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Ordered, additive migrations; entry `n` moves `user_version` from `n` to `n + 1`.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE requests (
        key TEXT PRIMARY KEY,
        root_session TEXT NOT NULL,
        repo TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        ended_at INTEGER
    );
    CREATE TABLE cursor (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        value INTEGER NOT NULL
    );
"];

/// Columns of a request row, in the order [`request_row`] reads them.
const REQUEST_COLUMNS: &str = "key, root_session, repo, created_at, ended_at";

/// One front request key and the root Session it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRow {
    /// The front's request key.
    pub key: String,
    /// The root Session the request started.
    pub root_session: SessionId,
    /// The repo the request runs against.
    pub repo: String,
    /// When the request was recorded.
    pub created_at: i64,
    /// When the request ended, if it has.
    pub ended_at: Option<i64>,
}

/// Why a database call failed.
#[derive(Debug)]
pub enum DbError {
    /// `SQLite` refused the statement.
    Sqlite(rusqlite::Error),
    /// The state directory or file could not be prepared.
    Io(io::Error),
    /// The file was written by a newer build.
    NewerSchema {
        /// The schema version in the file.
        found: i64,
        /// The newest version this build knows.
        known: i64,
    },
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "database error: {e}"),
            Self::Io(e) => write!(f, "state directory error: {e}"),
            Self::NewerSchema { found, known } => write!(
                f,
                "database schema {found} is newer than this build knows ({known})"
            ),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::NewerSchema { .. } => None,
        }
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<io::Error> for DbError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// The front's database, separate from the controller's.
pub struct Db {
    /// The single connection.
    conn: Connection,
}

impl Db {
    /// Open (creating if needed) the database at `path`. The state directory
    /// is created 0700 and the db, `-wal` and `-shm` files are 0600.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        prepare(path)?;
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        migrate(&mut conn)?;
        tighten(path)?;
        Ok(Self { conn })
    }

    /// Record a request; a key already present is an error.
    pub fn insert_request(&self, row: &RequestRow) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO requests (key, root_session, repo, created_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                row.key,
                row.root_session.get().to_string(),
                row.repo,
                row.created_at,
                row.ended_at
            ],
        )?;
        Ok(())
    }

    /// The request recorded under `key`.
    pub fn request(&self, key: &str) -> Result<Option<RequestRow>, DbError> {
        let sql = format!("SELECT {REQUEST_COLUMNS} FROM requests WHERE key = ?1");
        Ok(self.conn.query_row(&sql, [key], request_row).optional()?)
    }

    /// Every request, oldest first, ties broken by key.
    pub fn requests(&self) -> Result<Vec<RequestRow>, DbError> {
        let sql = format!("SELECT {REQUEST_COLUMNS} FROM requests ORDER BY created_at, key");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], request_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Mark a request ended; false when no request has that key.
    pub fn end_request(&self, key: &str, ended_at: i64) -> Result<bool, DbError> {
        let n = self.conn.execute(
            "UPDATE requests SET ended_at = ?2 WHERE key = ?1",
            params![key, ended_at],
        )?;
        Ok(n > 0)
    }

    /// The last Watch cursor, 0 when none was stored.
    pub fn cursor(&self) -> Result<i64, DbError> {
        let value = self
            .conn
            .query_row("SELECT value FROM cursor WHERE id = 0", [], |r| r.get(0))
            .optional()?;
        Ok(value.unwrap_or(0))
    }

    /// Store the last Watch cursor.
    pub fn set_cursor(&self, cursor: i64) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO cursor (id, value) VALUES (0, ?1)
             ON CONFLICT (id) DO UPDATE SET value = excluded.value",
            [cursor],
        )?;
        Ok(())
    }
}

/// Read one request row selected with [`REQUEST_COLUMNS`].
fn request_row(r: &Row<'_>) -> rusqlite::Result<RequestRow> {
    let text: String = r.get(1)?;
    let id = text
        .parse::<i64>()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(e)))?;
    Ok(RequestRow {
        key: r.get(0)?,
        root_session: SessionId::new(id),
        repo: r.get(2)?,
        created_at: r.get(3)?,
        ended_at: r.get(4)?,
    })
}

/// Apply the migrations not yet recorded in `user_version`, one transaction each.
fn migrate(conn: &mut Connection) -> Result<(), DbError> {
    let found: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let known = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    if found > known {
        return Err(DbError::NewerSchema { found, known });
    }
    let done = usize::try_from(found).unwrap_or(0);
    for (n, sql) in MIGRATIONS.iter().enumerate().skip(done) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        let version = i64::try_from(n + 1).unwrap_or(i64::MAX);
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

/// Create the state dir (0700) and the db file (0600) if missing, and tighten both if looser.
fn prepare(db: &Path) -> io::Result<()> {
    if let Some(dir) = db.parent().filter(|d| !d.as_os_str().is_empty()) {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    match OpenOptions::new()
        .create_new(true)
        .append(true)
        .mode(0o600)
        .open(db)
    {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    fs::set_permissions(db, fs::Permissions::from_mode(0o600))
}

/// Set the db file and its `-wal` and `-shm` siblings, where present, to 0600.
fn tighten(db: &Path) -> io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = db.as_os_str().to_owned();
        name.push(suffix);
        match fs::set_permissions(PathBuf::from(name), fs::Permissions::from_mode(0o600)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            other => other?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
