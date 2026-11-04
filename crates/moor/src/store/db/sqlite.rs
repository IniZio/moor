//! The `SQLite` backend: opening and migrating the file, transactions, rendering and binding
//! statements with `sea-query`'s `SQLite` builder, and reading its errors.

use std::fs::File;
use std::path::Path;
use std::time::Duration;

use moor_migrate::Outcome;
use rusqlite::types::{Value as Sql, ValueRef};
use rusqlite::{Connection, ErrorCode, OpenFlags};
use sea_query::SqliteQueryBuilder;

use super::{Db, Error, Result, Rows, Statement, UniqueKey, Value};
use crate::store::StoreError;

/// An open connection to the store's `SQLite` file.
pub(crate) struct Conn(Connection);

impl Conn {
    /// Open (creating if needed) the store's db at `path` as its writer, in WAL mode with
    /// foreign keys on, waiting `busy` on a lock, and migrated to this build's schema while
    /// holding what `claim` returns. The state dir ends 0700, the db, `-wal` and `-shm` 0600. A
    /// db this build would not migrate is refused before anything writes to it or changes a mode;
    /// one a newer build migrated opens unchanged, and the outcome says so.
    pub(super) fn writer(
        path: &Path,
        busy: Duration,
        claim: impl FnOnce(&Path) -> Result<Option<File>, StoreError>,
    ) -> Result<(Self, Outcome), StoreError> {
        migrate::refuse_other_version(path)?;
        files::prepare(path)?;
        let mut conn = Connection::open(path).map_err(error)?;
        conn.busy_timeout(busy).map_err(error)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )
        .map_err(error)?;
        let migrating = if migrate::is_latest(&conn, path)? {
            None
        } else {
            claim(path)?
        };
        let schema = migrate::migrate(&mut conn, path)?;
        drop(migrating);
        files::tighten(path)?;
        Ok((Self(conn), schema))
    }

    /// A read-only connection to `path`, waiting `busy` on a lock.
    pub(super) fn reader(path: &Path, busy: Duration) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(error)?;
        conn.busy_timeout(busy).map_err(error)?;
        Ok(Self(conn))
    }

    /// A plain read-write connection to the db file at `path`.
    #[cfg(test)]
    pub(super) fn open_file(path: &Path) -> Result<Self> {
        Connection::open(path).map(Self).map_err(error)
    }

    /// Begin a transaction that takes the write lock now.
    pub(super) fn immediate(&mut self) -> Result<Tx<'_>> {
        self.0
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map(Tx)
            .map_err(error)
    }

    /// Begin a transaction that takes the write lock at its first write.
    pub(super) fn transaction(&mut self) -> Result<Tx<'_>> {
        self.0.transaction().map(Tx).map_err(error)
    }

    /// Run several `;`-separated statements that bind nothing.
    #[cfg(test)]
    pub(super) fn execute_batch(&self, sql: &str) -> Result<()> {
        self.0.execute_batch(sql).map_err(error)
    }
}

/// Extended result code for a UNIQUE constraint failure.
const SQLITE_CONSTRAINT_UNIQUE: i32 = 2067;

/// `e` as the seam's error: a UNIQUE failure names its key, which `SQLite` writes as
/// `table.column, ..` after a fixed prefix.
pub(super) fn error(e: rusqlite::Error) -> Error {
    match unique_key(&e) {
        Some(key) => Error::Unique {
            key,
            message: e.to_string(),
        },
        None => Error::Backend(Box::new(e)),
    }
}

/// The UNIQUE key `e` reports a write collided with; none for any other error.
fn unique_key(e: &rusqlite::Error) -> Option<UniqueKey> {
    let rusqlite::Error::SqliteFailure(f, Some(msg)) = e else {
        return None;
    };
    if f.code != ErrorCode::ConstraintViolation || f.extended_code != SQLITE_CONSTRAINT_UNIQUE {
        return None;
    }
    let mut table = None;
    let mut columns = Vec::new();
    for key in msg.strip_prefix("UNIQUE constraint failed: ")?.split(", ") {
        let (t, c) = key.split_once('.')?;
        if *table.get_or_insert(t) != t {
            return None;
        }
        columns.push(c.to_owned());
    }
    Some(UniqueKey {
        table: table?.to_owned(),
        columns,
    })
}

/// An open transaction; dropping it rolls back.
pub(crate) struct Tx<'c>(rusqlite::Transaction<'c>);

impl Tx<'_> {
    /// Commit it.
    pub(super) fn commit(self) -> Result<()> {
        self.0.commit().map_err(error)
    }
}

/// `v` as the value rusqlite binds.
fn sql(v: Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Integer(i) => Sql::Integer(i),
        Value::Real(f) => Sql::Real(f),
        Value::Text(t) => Sql::Text(t),
        Value::Blob(b) => Sql::Blob(b),
    }
}

/// A column rusqlite read, owned; text that is not UTF-8 is refused.
fn owned(v: ValueRef<'_>) -> Result<Value> {
    Ok(match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Integer(i),
        ValueRef::Real(f) => Value::Real(f),
        ValueRef::Text(t) => Value::Text(
            std::str::from_utf8(t)
                .map_err(|e| Error::Backend(Box::new(e)))?
                .to_owned(),
        ),
        ValueRef::Blob(b) => Value::Blob(b.to_vec()),
    })
}

/// Run `s` on `conn`; the rows it changed.
fn execute(conn: &Connection, s: &Statement) -> Result<usize> {
    let (text, values) = s.render(SqliteQueryBuilder)?;
    conn.prepare_cached(&text)
        .and_then(|mut stmt| stmt.execute(rusqlite::params_from_iter(values.into_iter().map(sql))))
        .map_err(error)
}

/// Query `s` on `conn`, reading every row.
fn query(conn: &Connection, s: &Statement) -> Result<Rows> {
    let (text, values) = s.render(SqliteQueryBuilder)?;
    let mut stmt = conn.prepare_cached(&text).map_err(error)?;
    let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
    let mut rows = stmt
        .query(rusqlite::params_from_iter(values.into_iter().map(sql)))
        .map_err(error)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(error)? {
        let values = (0..names.len())
            .map(|i| owned(row.get_ref(i).map_err(error)?))
            .collect::<Result<_>>()?;
        out.push(values);
    }
    Ok(Rows { names, rows: out })
}

impl Db for Conn {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        execute(&self.0, s)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        query(&self.0, s)
    }
}

impl Db for Tx<'_> {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        execute(&self.0, s)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        query(&self.0, s)
    }
}

mod files;
mod migrate;

#[cfg(test)]
mod tests;
