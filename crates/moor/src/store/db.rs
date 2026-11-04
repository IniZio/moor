//! The store's database seam: a `sea-query` statement in, rows of plain values out.
//!
//! Each backend renders a [`Statement`] with its own query builder and binds its values, so
//! the store above this module never writes SQL or asks which database it runs on. A value
//! read back is a [`Value`], one of `SQLite`'s storage classes, so every backend reads a Rust
//! type the way the `SQLite` store always has, and fails with the same [`Error`].

use sea_query::{DeleteStatement, Iden, InsertStatement, SelectStatement, UpdateStatement};

/// A database operation's result.
pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

/// A failure a backend reports, kept for its message.
pub(crate) type Cause = Box<dyn std::error::Error + Send + Sync>;

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

impl Statement {
    /// Its SQL as `builder` renders it, and the values its placeholders bind.
    fn render(&self, builder: impl sea_query::QueryBuilder) -> Result<(String, Vec<Value>)> {
        let (sql, values) = match self {
            Self::Select(q) => q.build(builder),
            Self::Insert(q) => q.build(builder),
            Self::Update(q) => q.build(builder),
            Self::Delete(q) => q.build(builder),
        };
        let values = values.into_iter().map(bound).collect::<Result<_>>()?;
        Ok((sql, values))
    }
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
    /// Run `s`; the number of rows it changed.
    fn run_statement(&self, s: &Statement) -> Result<usize>;
    /// Run `s`; every row it returned.
    fn fetch_statement(&self, s: &Statement) -> Result<Rows>;
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
            Err(Error::NoColumn(self.to_string()))
        }
    }
}

impl RowIndex for &str {
    fn position(self, names: &[String]) -> Result<usize> {
        names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(self))
            .ok_or_else(|| Error::NoColumn(self.to_owned()))
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
    /// Column `idx` read as `T`.
    pub(crate) fn get<I: RowIndex, T: FromValue>(&self, idx: I) -> Result<T> {
        let index = idx.position(self.names)?;
        let value = self
            .values
            .get(index)
            .ok_or_else(|| Error::NoColumn(index.to_string()))?;
        T::from_value(value).map_err(|e| match e {
            Unread::Type => Error::Type {
                index,
                name: self.names.get(index).cloned().unwrap_or_default(),
                found: value.kind(),
            },
            Unread::OutOfRange(value) => Error::OutOfRange { index, value },
        })
    }
}

/// Rows mapped one at a time as they are taken.
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

/// The calls the store makes on a [`Db`].
pub(crate) trait DbExt: Db {
    /// Run `s`; the number of rows it changed.
    fn run(&self, s: impl Into<Statement>) -> Result<usize> {
        self.run_statement(&s.into())
    }

    /// Run `s`; its first row read by `f`, or [`Error::NoRows`].
    fn row<T, F>(&self, s: impl Into<Statement>, f: F) -> Result<T>
    where
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        let Rows { names, rows } = self.fetch_statement(&s.into())?;
        let values = rows.first().ok_or(Error::NoRows)?;
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

/// A UNIQUE key a write collided with, named by its table and columns as every backend reports
/// them, so the store tells one taken key from another without reading a backend's error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UniqueKey {
    /// The table the key is on.
    pub(crate) table: String,
    /// The key's columns, in key order.
    pub(crate) columns: Vec<String>,
}

impl UniqueKey {
    /// Whether this is the key on `columns` of `table`.
    pub(crate) fn is<C: Iden>(&self, table: &impl Iden, columns: &[C]) -> bool {
        self.table == table.to_string()
            && self.columns.len() == columns.len()
            && self
                .columns
                .iter()
                .zip(columns)
                .all(|(c, want)| *c == want.to_string())
    }
}

mod postgres;
mod sqlite;

mod conn;
mod error;
mod value;

pub(crate) use conn::{open_postgres, open_sqlite, Conn, Opened, Reader, Readers, Tx};
pub(crate) use error::{Error, OptionalExtension};
pub(crate) use postgres::Target;
use value::{bound, Unread};
pub(crate) use value::{FromValue, Kind, Value};

#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
