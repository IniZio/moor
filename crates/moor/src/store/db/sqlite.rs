//! The `SQLite` backend: opening and migrating the file, transactions, rendering and binding
//! statements with `sea-query`'s `SQLite` builder, and reading its errors.

use std::fs::File;
use std::path::Path;
use std::sync::MutexGuard;
use std::time::Duration;

use moor_migrate::Outcome;
use rusqlite::types::Value;
use rusqlite::{Connection, ErrorCode, OpenFlags};
use sea_query::SqliteQueryBuilder;

use super::{owned, Db, Error, Result, Rows, Statement, UniqueKey};
use crate::store::StoreError;

/// An open connection to the store's database.
pub(crate) struct Conn(Connection);

impl Conn {
    /// The `SQLite` connection `conn`.
    #[cfg(test)]
    pub(super) const fn sqlite(conn: Connection) -> Self {
        Self(conn)
    }

    /// Open (creating if needed) the store's `SQLite` db at `path` as its writer, in WAL mode
    /// with foreign keys on, waiting `busy` on a lock, and migrated to this build's schema while
    /// holding what `claim` returns. The state dir ends 0700, the db, `-wal` and `-shm` 0600. A db
    /// this build would not migrate is refused before anything writes to it or changes a mode; one
    /// a newer build migrated opens unchanged, and the outcome says so.
    pub(crate) fn sqlite_writer(
        path: &Path,
        busy: Duration,
        claim: impl FnOnce(&Path) -> Result<Option<File>, StoreError>,
    ) -> Result<(Self, Outcome), StoreError> {
        migrate::refuse_other_version(path)?;
        files::prepare(path)?;
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(busy)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )?;
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

    /// A read-only `SQLite` connection to `path`, waiting `busy` on a lock.
    pub(crate) fn sqlite_reader(path: &Path, busy: Duration) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(busy)?;
        Ok(Self(conn))
    }

    /// A plain read-write connection to the db file at `path`, as a test outside the store
    /// opens one beside a live store.
    #[cfg(test)]
    pub(crate) fn open_file(path: &Path) -> Result<Self> {
        Connection::open(path).map(Self)
    }

    /// Begin a transaction that takes the write lock now, so a read in it is not stale by the
    /// time it writes.
    pub(crate) fn immediate(&mut self) -> Result<Tx<'_>> {
        self.0
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map(Tx)
    }

    /// Begin a transaction that takes the write lock at its first write.
    pub(crate) fn transaction(&mut self) -> Result<Tx<'_>> {
        self.0.transaction().map(Tx)
    }

    /// Run several `;`-separated statements that bind nothing.
    #[cfg(test)]
    pub(super) fn execute_batch(&self, sql: &str) -> Result<()> {
        self.0.execute_batch(sql)
    }
}

/// Extended result code for a UNIQUE constraint failure.
const SQLITE_CONSTRAINT_UNIQUE: i32 = 2067;

/// The UNIQUE key `e` reports a write collided with; none for any other error. `SQLite` names
/// the key as `table.column, ..` after a fixed prefix.
pub(crate) fn unique_violation(e: &Error) -> Option<UniqueKey> {
    let Error::SqliteFailure(f, Some(msg)) = e else {
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
    pub(crate) fn commit(self) -> Result<()> {
        self.0.commit()
    }
}

/// `s` as `SQLite` SQL and the values its placeholders bind.
fn sqlite_render(s: &Statement) -> Result<(String, Vec<Value>)> {
    let (sql, values) = match s {
        Statement::Select(q) => q.build(SqliteQueryBuilder),
        Statement::Insert(q) => q.build(SqliteQueryBuilder),
        Statement::Update(q) => q.build(SqliteQueryBuilder),
        Statement::Delete(q) => q.build(SqliteQueryBuilder),
    };
    let values = values
        .into_iter()
        .map(sqlite_value)
        .collect::<Result<_>>()?;
    Ok((sql, values))
}

/// A `sea-query` value as the `SQLite` storage class it binds as.
fn sqlite_value(v: sea_query::Value) -> Result<Value> {
    use sea_query::Value as Q;
    let int = |i: Option<i64>| i.map_or(Value::Null, Value::Integer);
    Ok(match v {
        Q::Bool(b) => int(b.map(i64::from)),
        Q::TinyInt(i) => int(i.map(i64::from)),
        Q::SmallInt(i) => int(i.map(i64::from)),
        Q::Int(i) => int(i.map(i64::from)),
        Q::BigInt(i) => int(i),
        Q::TinyUnsigned(i) => int(i.map(i64::from)),
        Q::SmallUnsigned(i) => int(i.map(i64::from)),
        Q::Unsigned(i) => int(i.map(i64::from)),
        Q::BigUnsigned(i) => int(i
            .map(i64::try_from)
            .transpose()
            .map_err(|e| Error::ToSqlConversionFailure(e.into()))?),
        Q::Float(f) => f.map_or(Value::Null, |f| Value::Real(f.into())),
        Q::Double(f) => f.map_or(Value::Null, Value::Real),
        Q::String(t) => t.map_or(Value::Null, Value::Text),
        Q::Char(c) => c.map_or(Value::Null, |c| Value::Text(c.into())),
        Q::Bytes(b) => b.map_or(Value::Null, Value::Blob),
        Q::Enum(_) => return Err(Error::ToSqlConversionFailure("enum values".into())),
    })
}

/// Run `sql` on the `SQLite` connection `conn`.
fn sqlite_execute(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<usize> {
    conn.prepare_cached(sql)?
        .execute(rusqlite::params_from_iter(params))
}

/// Query `sql` on the `SQLite` connection `conn`, reading every row.
fn sqlite_query(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Rows> {
    let mut stmt = conn.prepare_cached(sql)?;
    let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
    let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let values = (0..names.len())
            .map(|i| owned(row.get_ref(i)?))
            .collect::<Result<_>>()?;
        out.push(values);
    }
    Ok(Rows { names, rows: out })
}

impl Db for Conn {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        let (sql, params) = sqlite_render(s)?;
        sqlite_execute(&self.0, &sql, params)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        let (sql, params) = sqlite_render(s)?;
        sqlite_query(&self.0, &sql, params)
    }
}

impl Db for Tx<'_> {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        let (sql, params) = sqlite_render(s)?;
        sqlite_execute(&self.0, &sql, params)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        let (sql, params) = sqlite_render(s)?;
        sqlite_query(&self.0, &sql, params)
    }
}

impl Db for MutexGuard<'_, Conn> {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        (**self).run_statement(s)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        (**self).fetch_statement(s)
    }
}

mod files;
mod migrate;

#[cfg(test)]
mod tests;
