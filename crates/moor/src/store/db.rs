//! The store's database seam: SQL text and plain values in, rows of plain values out.
//!
//! A value is one of `SQLite`'s storage classes, and binding and reading convert through
//! rusqlite's `ToSql` and `FromSql`, so every backend binds and reads a Rust type the way the
//! `SQLite` store always has. SQL is written for `SQLite`; the Postgres backend rewrites its
//! placeholders.

use std::path::Path;
use std::sync::MutexGuard;

use rusqlite::types::{FromSql, FromSqlError, Value, ValueRef};
use rusqlite::{Connection, OpenFlags};

pub(crate) use moor_migrate::Backend;
pub(crate) use rusqlite::{params, ToSql, TransactionBehavior};

mod params;
mod postgres;

use params::owned;
pub(crate) use params::Params;

pub(crate) use postgres::PgConn;

/// Why a database operation failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// `SQLite` failed, or a value did not bind or read under rusqlite's rules, on any backend.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Postgres failed.
    #[error("postgres: {0}")]
    Postgres(#[from] ::postgres::Error),
}

impl Error {
    /// Whether this is a unique-constraint failure: on `SQLite` one whose message `sqlite`
    /// accepts, on Postgres one on the constraint or index named `postgres`.
    pub(crate) fn is_unique_violation(
        &self,
        sqlite: impl Fn(&str) -> bool,
        postgres: &str,
    ) -> bool {
        match self {
            Self::Sqlite(rusqlite::Error::SqliteFailure(f, msg)) => {
                f.code == rusqlite::ErrorCode::ConstraintViolation
                    && f.extended_code == SQLITE_CONSTRAINT_UNIQUE
                    && msg.as_deref().is_some_and(sqlite)
            }
            Self::Sqlite(_) => false,
            Self::Postgres(e) => e.as_db_error().is_some_and(|db| {
                *db.code() == ::postgres::error::SqlState::UNIQUE_VIOLATION
                    && db.constraint() == Some(postgres)
            }),
        }
    }
}

/// `SQLite`'s extended result code for a UNIQUE constraint failure.
const SQLITE_CONSTRAINT_UNIQUE: i32 = 2067;

/// A database operation's result.
pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

/// `query_row`'s missing row as `None`, as rusqlite's `OptionalExtension` does.
pub(crate) trait OptionalExtension<T> {
    /// `Ok(None)` for no row; other results unchanged.
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(Error::Sqlite(rusqlite::Error::QueryReturnedNoRows)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// What a query returned: its column names and its rows, in order.
#[derive(Debug, Default)]
pub(crate) struct Rows {
    /// Column names, in select order.
    names: Vec<String>,
    /// One value per column for each row.
    rows: Vec<Vec<Value>>,
}

/// Something SQL runs on: a connection or an open transaction.
pub(crate) trait Db {
    /// Run `sql` with `params` bound; the number of rows it changed.
    fn execute_values(&self, sql: &str, params: Vec<Value>) -> Result<usize>;
    /// Run `sql` with `params` bound; every row it returned.
    fn query_values(&self, sql: &str, params: Vec<Value>) -> Result<Rows>;
    /// The database it runs on, for the few statements written per backend.
    fn backend(&self) -> Backend;
}

/// A column of a row: its position, or its name.
pub(crate) trait RowIndex {
    /// The column's position in `names`.
    fn position(self, names: &[String]) -> rusqlite::Result<usize>;
}

impl RowIndex for usize {
    fn position(self, names: &[String]) -> rusqlite::Result<usize> {
        if self < names.len() {
            Ok(self)
        } else {
            Err(rusqlite::Error::InvalidColumnIndex(self))
        }
    }
}

impl RowIndex for &str {
    fn position(self, names: &[String]) -> rusqlite::Result<usize> {
        names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(self))
            .ok_or_else(|| rusqlite::Error::InvalidColumnName(self.to_owned()))
    }
}

/// One returned row.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Row<'a> {
    /// The query's column names.
    names: &'a [String],
    /// This row's values, one per column.
    values: &'a [Value],
}

impl Row<'_> {
    /// Column `idx` read as `T`, with rusqlite's conversion rules and errors.
    pub(crate) fn get<I: RowIndex, T: FromSql>(&self, idx: I) -> rusqlite::Result<T> {
        use rusqlite::Error as E;
        let i = idx.position(self.names)?;
        let value = self.values.get(i).ok_or(E::InvalidColumnIndex(i))?;
        let value = ValueRef::from(value);
        T::column_result(value).map_err(|e| match e {
            FromSqlError::InvalidType => E::InvalidColumnType(
                i,
                self.names.get(i).cloned().unwrap_or_default(),
                value.data_type(),
            ),
            FromSqlError::OutOfRange(n) => E::IntegralValueOutOfRange(i, n),
            FromSqlError::Utf8Error(e) => E::Utf8Error(i, e),
            other => E::FromSqlConversionFailure(i, value.data_type(), Box::new(other)),
        })
    }
}

/// Rows mapped one at a time as they are taken, like rusqlite's mapped rows.
pub(crate) struct Mapped<F> {
    /// The query's column names.
    names: Vec<String>,
    /// The rows not taken yet.
    rows: std::vec::IntoIter<Vec<Value>>,
    /// The mapping applied to each row.
    f: F,
}

impl<T, E, F: FnMut(&Row<'_>) -> Result<T, E>> Iterator for Mapped<F> {
    type Item = Result<T, E>;

    fn next(&mut self) -> Option<Self::Item> {
        let values = self.rows.next()?;
        Some((self.f)(&Row {
            names: &self.names,
            values: &values,
        }))
    }
}

/// A statement to run, possibly more than once, on `db`.
pub(crate) struct Stmt<'a, D: ?Sized> {
    /// Where it runs.
    db: &'a D,
    /// Its SQL.
    sql: String,
}

impl<D: Db + ?Sized> Stmt<'_, D> {
    /// Run it; the rows mapped lazily by `f`, which reads columns.
    pub(crate) fn query_map<T, P, F>(&mut self, p: P, f: F) -> Result<Mapped<F>>
    where
        P: Params,
        F: FnMut(&Row<'_>) -> rusqlite::Result<T>,
    {
        self.query_and_then(p, f)
    }

    /// Run it; the rows mapped lazily by `f`, whose error is the caller's.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "rusqlite's signature, so a statement is still bound `mut` and run more than once"
    )]
    pub(crate) fn query_and_then<T, E, P, F>(&mut self, p: P, f: F) -> Result<Mapped<F>>
    where
        P: Params,
        F: FnMut(&Row<'_>) -> Result<T, E>,
    {
        let Rows { names, rows } = self.db.query_values(&self.sql, p.values()?)?;
        Ok(Mapped {
            names,
            rows: rows.into_iter(),
            f,
        })
    }
}

/// The calls the store makes on a [`Db`], named as rusqlite names them.
pub(crate) trait DbExt: Db {
    /// Run `sql`; the number of rows it changed.
    fn execute<P: Params>(&self, sql: &str, p: P) -> Result<usize> {
        self.execute_values(sql, p.values()?)
    }

    /// Run `sql`; its first row read by `f`, or `QueryReturnedNoRows`.
    fn query_row<T, P, F>(&self, sql: &str, p: P, f: F) -> Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        let Rows { names, rows } = self.query_values(sql, p.values()?)?;
        let values = rows.first().ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        Ok(f(&Row {
            names: &names,
            values,
        })?)
    }

    /// `sql` ready to run.
    fn prepare(&self, sql: &str) -> Result<Stmt<'_, Self>> {
        Ok(Stmt {
            db: self,
            sql: sql.to_owned(),
        })
    }
}

impl<D: Db + ?Sized> DbExt for D {}

/// An open connection to the store's database.
pub(crate) enum Conn {
    /// A `SQLite` file.
    Sqlite(Connection),
    /// A Postgres database.
    Postgres(Box<PgConn>),
}

impl Conn {
    /// The `SQLite` connection `conn`, already opened and migrated.
    pub(crate) const fn sqlite(conn: Connection) -> Self {
        Self::Sqlite(conn)
    }

    /// A read-only `SQLite` connection to `path`, waiting `busy` on a lock.
    pub(crate) fn sqlite_reader(path: &Path, busy: std::time::Duration) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(busy)?;
        Ok(Self::Sqlite(conn))
    }

    /// Begin a transaction that takes the write lock as `behavior` says. On Postgres an
    /// immediate one holds the store's advisory lock, so immediate transactions run one at a time
    /// as they do on `SQLite`.
    pub(crate) fn transaction_with_behavior(
        &mut self,
        behavior: TransactionBehavior,
    ) -> Result<Tx<'_>> {
        match self {
            Self::Sqlite(c) => Ok(Tx::Sqlite(c.transaction_with_behavior(behavior)?)),
            Self::Postgres(c) => Ok(Tx::Postgres(
                c.transaction(matches!(behavior, TransactionBehavior::Immediate))?,
            )),
        }
    }

    /// Begin a transaction that takes the write lock at its first write.
    pub(crate) fn transaction(&mut self) -> Result<Tx<'_>> {
        self.transaction_with_behavior(TransactionBehavior::Deferred)
    }

    /// Run several `;`-separated statements that bind nothing.
    #[cfg(test)]
    pub(crate) fn execute_batch(&self, sql: &str) -> Result<()> {
        match self {
            Self::Sqlite(c) => Ok(c.execute_batch(sql)?),
            Self::Postgres(c) => c.execute_batch(sql),
        }
    }
}

/// An open transaction; dropping it rolls back.
pub(crate) enum Tx<'c> {
    /// On a `SQLite` connection.
    Sqlite(rusqlite::Transaction<'c>),
    /// On a Postgres connection.
    Postgres(postgres::PgTx<'c>),
}

impl Tx<'_> {
    /// Commit it.
    pub(crate) fn commit(self) -> Result<()> {
        match self {
            Self::Sqlite(tx) => Ok(tx.commit()?),
            Self::Postgres(tx) => tx.commit(),
        }
    }
}

/// Run `sql` on the `SQLite` connection `conn`.
fn sqlite_execute(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<usize> {
    Ok(conn
        .prepare_cached(sql)?
        .execute(rusqlite::params_from_iter(params))?)
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
            .collect::<rusqlite::Result<_>>()?;
        out.push(values);
    }
    Ok(Rows { names, rows: out })
}

impl Db for Conn {
    fn execute_values(&self, sql: &str, params: Vec<Value>) -> Result<usize> {
        match self {
            Self::Sqlite(c) => sqlite_execute(c, sql, params),
            Self::Postgres(c) => c.execute(sql, &params),
        }
    }

    fn query_values(&self, sql: &str, params: Vec<Value>) -> Result<Rows> {
        match self {
            Self::Sqlite(c) => sqlite_query(c, sql, params),
            Self::Postgres(c) => c.query(sql, &params),
        }
    }

    fn backend(&self) -> Backend {
        match self {
            Self::Sqlite(_) => Backend::Sqlite,
            Self::Postgres(_) => Backend::Postgres,
        }
    }
}

impl Db for Tx<'_> {
    fn execute_values(&self, sql: &str, params: Vec<Value>) -> Result<usize> {
        match self {
            Self::Sqlite(tx) => sqlite_execute(tx, sql, params),
            Self::Postgres(tx) => tx.execute(sql, &params),
        }
    }

    fn query_values(&self, sql: &str, params: Vec<Value>) -> Result<Rows> {
        match self {
            Self::Sqlite(tx) => sqlite_query(tx, sql, params),
            Self::Postgres(tx) => tx.query(sql, &params),
        }
    }

    fn backend(&self) -> Backend {
        match self {
            Self::Sqlite(_) => Backend::Sqlite,
            Self::Postgres(_) => Backend::Postgres,
        }
    }
}

impl Db for MutexGuard<'_, Conn> {
    fn execute_values(&self, sql: &str, params: Vec<Value>) -> Result<usize> {
        (**self).execute_values(sql, params)
    }

    fn query_values(&self, sql: &str, params: Vec<Value>) -> Result<Rows> {
        (**self).query_values(sql, params)
    }

    fn backend(&self) -> Backend {
        (**self).backend()
    }
}

#[cfg(test)]
mod tests;
