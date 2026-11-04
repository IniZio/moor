//! The store's database seam: a `sea-query` statement in, rows of plain values out.
//!
//! Each backend renders a [`Statement`] with its own query builder and binds its values, so
//! the store above this module never writes SQL or asks which database it runs on. A value
//! read back is one of `SQLite`'s storage classes, read through rusqlite's `FromSql`, so every
//! backend reads a Rust type the way the `SQLite` store always has.

use std::path::Path;
use std::sync::MutexGuard;

use rusqlite::types::{FromSql, FromSqlError, ToSqlOutput, Value, ValueRef};
use rusqlite::{Connection, OpenFlags};
use sea_query::{
    DeleteStatement, InsertStatement, SelectStatement, SqliteQueryBuilder, UpdateStatement,
};

pub(crate) use rusqlite::{params, Error, OptionalExtension, ToSql, TransactionBehavior};

/// A database operation's result.
pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

/// What a query returned: its column names and its rows, in order.
#[derive(Debug, Default)]
pub(crate) struct Rows {
    /// Column names, in select order.
    names: Vec<String>,
    /// One value per column for each row.
    rows: Vec<Vec<Value>>,
}

/// A statement the store runs, built with `sea-query` and rendered by the backend.
#[derive(Debug, Clone)]
pub(crate) enum Statement {
    /// A `SELECT`.
    Select(SelectStatement),
    /// An `INSERT`, maybe with `ON CONFLICT` or `RETURNING`.
    Insert(InsertStatement),
    /// An `UPDATE`.
    Update(UpdateStatement),
    /// A `DELETE`.
    Delete(DeleteStatement),
}

/// `From` a statement, owned or as the `&mut` a builder chain ends on.
macro_rules! statement_from {
    ($($variant:ident($ty:ty))+) => {$(
        impl From<$ty> for Statement {
            fn from(s: $ty) -> Self {
                Self::$variant(s)
            }
        }

        impl From<&mut $ty> for Statement {
            fn from(s: &mut $ty) -> Self {
                Self::$variant(s.take())
            }
        }
    )+};
}
statement_from! {
    Select(SelectStatement)
    Insert(InsertStatement)
    Update(UpdateStatement)
    Delete(DeleteStatement)
}

/// `q` as a value: the one column of its one row.
pub(crate) fn sub(q: SelectStatement) -> sea_query::Expr {
    sea_query::Expr::SubQuery(None, Box::new(q.into()))
}

/// Something SQL runs on: a connection or an open transaction.
pub(crate) trait Db {
    /// Run `sql` with `params` bound; the number of rows it changed.
    fn execute_values(&self, sql: &str, params: Vec<Value>) -> Result<usize>;
    /// Run `sql` with `params` bound; every row it returned.
    fn query_values(&self, sql: &str, params: Vec<Value>) -> Result<Rows>;
    /// Run `s`; the number of rows it changed.
    fn run_statement(&self, s: &Statement) -> Result<usize>;
    /// Run `s`; every row it returned.
    fn fetch_statement(&self, s: &Statement) -> Result<Rows>;
}

/// The values bound to a statement's placeholders, in order.
pub(crate) trait Params {
    /// The bound values.
    fn values(self) -> Result<Vec<Value>>;
}

/// One bound value, converted the way rusqlite binds it.
fn bound(p: &(impl ToSql + ?Sized)) -> Result<Value> {
    Ok(match p.to_sql()? {
        ToSqlOutput::Borrowed(v) => owned(v)?,
        ToSqlOutput::Owned(v) => v,
        _ => return Err(Error::ToSqlConversionFailure("unsupported value".into())),
    })
}

impl Params for &[&dyn ToSql] {
    fn values(self) -> Result<Vec<Value>> {
        self.iter().map(|p| bound(*p)).collect()
    }
}

impl Params for Vec<Value> {
    fn values(self) -> Result<Vec<Value>> {
        Ok(self)
    }
}

impl Params for () {
    fn values(self) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }
}

impl Params for [&(dyn ToSql + Send + Sync); 0] {
    fn values(self) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }
}

/// `Params` for arrays of one to `N` values.
macro_rules! array_params {
    ($($n:literal)+) => {$(
        impl<T: ToSql> Params for [T; $n] {
            fn values(self) -> Result<Vec<Value>> {
                self.iter().map(bound).collect()
            }
        }
    )+};
}
array_params!(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16);

/// `Params` for tuples of one to sixteen values.
macro_rules! tuple_params {
    ($(($($t:ident $i:tt),+))+) => {$(
        impl<$($t: ToSql),+> Params for ($($t,)+) {
            fn values(self) -> Result<Vec<Value>> {
                Ok(vec![$(bound(&self.$i)?),+])
            }
        }
    )+};
}
tuple_params! {
    (A 0)
    (A 0, B 1)
    (A 0, B 1, C 2)
    (A 0, B 1, C 2, D 3)
    (A 0, B 1, C 2, D 3, E 4)
    (A 0, B 1, C 2, D 3, E 4, F 5)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11)
}

/// A column of a row: its position, or its name.
pub(crate) trait RowIndex {
    /// The column's position in `names`.
    fn position(self, names: &[String]) -> Result<usize>;
}

impl RowIndex for usize {
    fn position(self, names: &[String]) -> Result<usize> {
        if self < names.len() {
            Ok(self)
        } else {
            Err(Error::InvalidColumnIndex(self))
        }
    }
}

impl RowIndex for &str {
    fn position(self, names: &[String]) -> Result<usize> {
        names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(self))
            .ok_or_else(|| Error::InvalidColumnName(self.to_owned()))
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
    pub(crate) fn get<I: RowIndex, T: FromSql>(&self, idx: I) -> Result<T> {
        let i = idx.position(self.names)?;
        let value = self.values.get(i).ok_or(Error::InvalidColumnIndex(i))?;
        let value = ValueRef::from(value);
        T::column_result(value).map_err(|e| match e {
            FromSqlError::InvalidType => Error::InvalidColumnType(
                i,
                self.names.get(i).cloned().unwrap_or_default(),
                value.data_type(),
            ),
            FromSqlError::OutOfRange(n) => Error::IntegralValueOutOfRange(i, n),
            FromSqlError::Utf8Error(e) => Error::Utf8Error(i, e),
            other => Error::FromSqlConversionFailure(i, value.data_type(), Box::new(other)),
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
        F: FnMut(&Row<'_>) -> Result<T>,
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
        E: From<Error>,
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
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        let Rows { names, rows } = self.query_values(sql, p.values()?)?;
        let values = rows.first().ok_or(Error::QueryReturnedNoRows)?;
        f(&Row {
            names: &names,
            values,
        })
    }

    /// `sql` ready to run.
    fn prepare(&self, sql: &str) -> Result<Stmt<'_, Self>> {
        Ok(Stmt {
            db: self,
            sql: sql.to_owned(),
        })
    }

    /// Run `s`; the number of rows it changed.
    fn run(&self, s: impl Into<Statement>) -> Result<usize> {
        self.run_statement(&s.into())
    }

    /// Run `s`; its first row read by `f`, or `QueryReturnedNoRows`.
    fn row<T, F>(&self, s: impl Into<Statement>, f: F) -> Result<T>
    where
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        let Rows { names, rows } = self.fetch_statement(&s.into())?;
        let values = rows.first().ok_or(Error::QueryReturnedNoRows)?;
        f(&Row {
            names: &names,
            values,
        })
    }

    /// Run `s`; its rows mapped lazily by `f`, whose error is the caller's.
    fn rows<T, E, F>(&self, s: impl Into<Statement>, f: F) -> Result<Mapped<F>>
    where
        E: From<Error>,
        F: FnMut(&Row<'_>) -> Result<T, E>,
    {
        let Rows { names, rows } = self.fetch_statement(&s.into())?;
        Ok(Mapped {
            names,
            rows: rows.into_iter(),
            f,
        })
    }
}

impl<D: Db + ?Sized> DbExt for D {}

/// The `SQLite` writer connection to `path`, in WAL mode with foreign keys on, waiting `busy`
/// on a lock.
pub(crate) fn sqlite_writer(path: &Path, busy: std::time::Duration) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(busy)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
    )?;
    Ok(conn)
}

/// An open connection to the store's database.
pub(crate) struct Conn(Connection);

impl Conn {
    /// The `SQLite` connection `conn`, already opened and migrated.
    pub(crate) const fn sqlite(conn: Connection) -> Self {
        Self(conn)
    }

    /// A read-only `SQLite` connection to `path`, waiting `busy` on a lock.
    pub(crate) fn sqlite_reader(path: &Path, busy: std::time::Duration) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(busy)?;
        Ok(Self(conn))
    }

    /// Begin a transaction that takes the write lock as `behavior` says.
    pub(crate) fn transaction_with_behavior(
        &mut self,
        behavior: TransactionBehavior,
    ) -> Result<Tx<'_>> {
        self.0.transaction_with_behavior(behavior).map(Tx)
    }

    /// Begin a transaction that takes the write lock at its first write.
    pub(crate) fn transaction(&mut self) -> Result<Tx<'_>> {
        self.0.transaction().map(Tx)
    }

    /// Run several `;`-separated statements that bind nothing.
    #[cfg(test)]
    pub(crate) fn execute_batch(&self, sql: &str) -> Result<()> {
        self.0.execute_batch(sql)
    }
}

/// An open transaction; dropping it rolls back.
pub(crate) struct Tx<'c>(rusqlite::Transaction<'c>);

impl Tx<'_> {
    /// Commit it.
    pub(crate) fn commit(self) -> Result<()> {
        self.0.commit()
    }
}

/// `v` as an owned value; text that is not UTF-8 is refused as rusqlite refuses it.
fn owned(v: ValueRef<'_>) -> Result<Value> {
    Ok(match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Integer(i),
        ValueRef::Real(f) => Value::Real(f),
        ValueRef::Text(t) => Value::Text(
            std::str::from_utf8(t)
                .map_err(|e| Error::ToSqlConversionFailure(e.into()))?
                .to_owned(),
        ),
        ValueRef::Blob(b) => Value::Blob(b.to_vec()),
    })
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
    fn execute_values(&self, sql: &str, params: Vec<Value>) -> Result<usize> {
        sqlite_execute(&self.0, sql, params)
    }

    fn query_values(&self, sql: &str, params: Vec<Value>) -> Result<Rows> {
        sqlite_query(&self.0, sql, params)
    }

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
    fn execute_values(&self, sql: &str, params: Vec<Value>) -> Result<usize> {
        sqlite_execute(&self.0, sql, params)
    }

    fn query_values(&self, sql: &str, params: Vec<Value>) -> Result<Rows> {
        sqlite_query(&self.0, sql, params)
    }

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
    fn execute_values(&self, sql: &str, params: Vec<Value>) -> Result<usize> {
        (**self).execute_values(sql, params)
    }

    fn query_values(&self, sql: &str, params: Vec<Value>) -> Result<Rows> {
        (**self).query_values(sql, params)
    }

    fn run_statement(&self, s: &Statement) -> Result<usize> {
        (**self).run_statement(s)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        (**self).fetch_statement(s)
    }
}

#[cfg(test)]
mod tests;
