//! The seam on Postgres: the store's `SQLite` placeholders rewritten to `$N`, each value bound as
//! the type Postgres infers for its parameter, and each column read back as a `SQLite` storage
//! class, so callers' `ToSql` and `FromSql` conversions behave as they do on `SQLite`.

use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;

use bytes::BytesMut;
use postgres::types::{to_sql_checked, FromSql, IsNull, ToSql, Type};
use postgres::{Client, GenericClient, Statement, Transaction};
use rusqlite::types::Value;

use super::{Result, Rows};
use target::Target;

/// A conversion's failure, as `postgres-types` carries it.
type Boxed = Box<dyn StdError + Sync + Send>;

/// Taken by every immediate transaction, keyed by schema so stores in separate schemas of one
/// database never wait on each other.
const IMMEDIATE_LOCK: &str = "SELECT pg_advisory_xact_lock(hashtext(current_schema()), 1)";

/// A Postgres connection and the statements it has prepared, by their `SQLite` text. A call
/// on a connection that closed, as when the server restarted, connects again first; the call
/// that found it closed failed and is not run again.
pub(crate) struct PgConn {
    /// Where it connects.
    target: Arc<Target>,
    /// The connection.
    client: RefCell<Client>,
    /// Prepared statements, reused for the connection's life.
    prepared: RefCell<HashMap<String, Statement>>,
}

/// A Postgres transaction, preparing through its connection's statement cache.
pub(crate) struct PgTx<'c> {
    /// The transaction.
    tx: RefCell<Transaction<'c>>,
    /// The connection's prepared statements.
    prepared: &'c RefCell<HashMap<String, Statement>>,
}

impl PgConn {
    /// A new connection to `target`.
    pub(crate) fn connect(target: Arc<Target>) -> Result<Self> {
        Ok(Self {
            client: RefCell::new(target.connect()?),
            target,
            prepared: RefCell::default(),
        })
    }

    /// Whether the connection has closed.
    pub(crate) fn is_closed(&self) -> bool {
        self.client.borrow().is_closed()
    }

    /// The connection, made again first when it has closed.
    fn client(&self) -> Result<RefMut<'_, Client>> {
        if self.is_closed() {
            *self.client.borrow_mut() = self.target.connect()?;
            self.prepared.borrow_mut().clear();
        }
        Ok(self.client.borrow_mut())
    }

    /// Begin a transaction; an `immediate` one first takes the store's advisory lock.
    pub(super) fn transaction(&mut self, immediate: bool) -> Result<PgTx<'_>> {
        drop(self.client()?);
        let Self {
            client, prepared, ..
        } = self;
        let mut tx = client.get_mut().transaction()?;
        if immediate {
            tx.batch_execute(IMMEDIATE_LOCK)?;
        }
        Ok(PgTx {
            tx: RefCell::new(tx),
            prepared,
        })
    }

    /// Run `sql` with `params`; the rows it changed.
    pub(super) fn execute(&self, sql: &str, params: &[Value]) -> Result<usize> {
        execute(&mut *self.client()?, &self.prepared, sql, params)
    }

    /// Query `sql` with `params`; every row.
    pub(super) fn query(&self, sql: &str, params: &[Value]) -> Result<Rows> {
        query(&mut *self.client()?, &self.prepared, sql, params)
    }

    /// Run several statements that bind nothing.
    #[cfg(test)]
    pub(super) fn execute_batch(&self, sql: &str) -> Result<()> {
        Ok(self.client()?.batch_execute(sql)?)
    }
}

impl PgTx<'_> {
    /// Run `sql` with `params`; the rows it changed.
    pub(super) fn execute(&self, sql: &str, params: &[Value]) -> Result<usize> {
        execute(&mut *self.tx.borrow_mut(), self.prepared, sql, params)
    }

    /// Query `sql` with `params`; every row.
    pub(super) fn query(&self, sql: &str, params: &[Value]) -> Result<Rows> {
        query(&mut *self.tx.borrow_mut(), self.prepared, sql, params)
    }

    /// Commit it.
    pub(super) fn commit(self) -> Result<()> {
        Ok(self.tx.into_inner().commit()?)
    }
}

/// `sql` prepared on `db`, from `cache` when it was prepared before.
fn prepared(
    db: &mut impl GenericClient,
    cache: &RefCell<HashMap<String, Statement>>,
    sql: &str,
) -> Result<Statement> {
    if let Some(stmt) = cache.borrow().get(sql) {
        return Ok(stmt.clone());
    }
    let stmt = db.prepare(&placeholders(sql))?;
    cache.borrow_mut().insert(sql.to_owned(), stmt.clone());
    Ok(stmt)
}

/// `params` as Postgres arguments.
fn args(params: &[Value]) -> Vec<Bind<'_>> {
    params.iter().map(Bind).collect()
}

/// Run `sql` on `db`; the rows it changed.
fn execute(
    db: &mut impl GenericClient,
    cache: &RefCell<HashMap<String, Statement>>,
    sql: &str,
    params: &[Value],
) -> Result<usize> {
    let stmt = prepared(db, cache, sql)?;
    let args = args(params);
    let refs: Vec<&(dyn ToSql + Sync)> = args.iter().map(|a| a as _).collect();
    let n = db.execute(&stmt, &refs)?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

/// Query `sql` on `db`, reading every row as `SQLite` values.
fn query(
    db: &mut impl GenericClient,
    cache: &RefCell<HashMap<String, Statement>>,
    sql: &str,
    params: &[Value],
) -> Result<Rows> {
    let stmt = prepared(db, cache, sql)?;
    let args = args(params);
    let refs: Vec<&(dyn ToSql + Sync)> = args.iter().map(|a| a as _).collect();
    let names = stmt.columns().iter().map(|c| c.name().to_owned()).collect();
    let rows = db
        .query(&stmt, &refs)?
        .iter()
        .map(|row| {
            (0..row.len())
                .map(|i| row.try_get::<_, Read>(i).map(|r| r.0))
                .collect::<std::result::Result<_, _>>()
        })
        .collect::<std::result::Result<_, _>>()?;
    Ok(Rows { names, rows })
}

/// `sql` with `SQLite`'s `?` and `?N` placeholders written as Postgres's `$N`. A bare `?` takes
/// the number after the largest one so far, as `SQLite` numbers it. Quoted text, quoted names
/// and comments are copied as they are.
pub(super) fn placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut largest = 0u32;
    while let Some(c) = chars.next() {
        out.push(c);
        let close = match (c, chars.peek()) {
            ('\'', _) => "'",
            ('"', _) => "\"",
            ('-', Some('-')) => "\n",
            ('/', Some('*')) => {
                out.extend(chars.next());
                "*/"
            }
            ('?', _) => {
                out.pop();
                let mut digits = String::new();
                while let Some(d) = chars.next_if(char::is_ascii_digit) {
                    digits.push(d);
                }
                let n = digits.parse().unwrap_or(largest + 1);
                largest = largest.max(n);
                out.push('$');
                out.push_str(&n.to_string());
                continue;
            }
            _ => continue,
        };
        copy_through(&mut chars, &mut out, close);
    }
    out
}

/// Copy `chars` into `out` up to and including `close`, or to the end.
fn copy_through(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut String,
    close: &str,
) {
    let mut copied = String::new();
    for c in chars.by_ref() {
        out.push(c);
        copied.push(c);
        if copied.ends_with(close) {
            return;
        }
    }
}

/// A `SQLite` value bound as the type Postgres inferred for its parameter.
#[derive(Debug)]
struct Bind<'a>(&'a Value);

impl ToSql for Bind<'_> {
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> std::result::Result<IsNull, Boxed> {
        match (self.0, ty) {
            (Value::Null, _) => Ok(IsNull::Yes),
            (Value::Integer(i), &Type::INT8) => i.to_sql(ty, out),
            (Value::Integer(i), &Type::INT4) => i32::try_from(*i)?.to_sql(ty, out),
            (Value::Integer(i), &Type::INT2) => i16::try_from(*i)?.to_sql(ty, out),
            (Value::Integer(i), &Type::BOOL) => (*i != 0).to_sql(ty, out),
            (Value::Real(f), &Type::FLOAT8) => f.to_sql(ty, out),
            (Value::Text(s), ty) if <&str as ToSql>::accepts(ty) => s.as_str().to_sql(ty, out),
            (Value::Blob(b), &Type::BYTEA) => b.as_slice().to_sql(ty, out),
            (v, ty) => Err(format!("cannot bind {v:?} as {ty}").into()),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

/// A Postgres column read as the `SQLite` storage class that holds it.
struct Read(Value);

impl<'a> FromSql<'a> for Read {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> std::result::Result<Self, Boxed> {
        let value = match *ty {
            Type::INT8 => Value::Integer(i64::from_sql(ty, raw)?),
            Type::INT4 => Value::Integer(i32::from_sql(ty, raw)?.into()),
            Type::INT2 => Value::Integer(i16::from_sql(ty, raw)?.into()),
            Type::BOOL => Value::Integer(bool::from_sql(ty, raw)?.into()),
            Type::FLOAT8 => Value::Real(f64::from_sql(ty, raw)?),
            Type::FLOAT4 => Value::Real(f32::from_sql(ty, raw)?.into()),
            Type::NUMERIC => numeric(raw)?,
            Type::BYTEA => Value::Blob(Vec::<u8>::from_sql(ty, raw)?),
            _ if <String as FromSql>::accepts(ty) => Value::Text(String::from_sql(ty, raw)?),
            _ => return Err(format!("cannot read a {ty} column").into()),
        };
        Ok(Self(value))
    }

    fn from_sql_null(_: &Type) -> std::result::Result<Self, Boxed> {
        Ok(Self(Value::Null))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

/// A binary `numeric`, as an integer when it is whole and fits, as `SUM` over integers is on
/// `SQLite`, and as a real otherwise.
fn numeric(raw: &[u8]) -> std::result::Result<Value, Boxed> {
    let word = |i: usize| -> std::result::Result<i16, Boxed> {
        match raw.get(i * 2..i * 2 + 2) {
            Some(&[hi, lo]) => Ok(i16::from_be_bytes([hi, lo])),
            _ => Err("short numeric".into()),
        }
    };
    let ndigits = word(0)?;
    let weight = i32::from(word(1)?);
    let negative = match word(2)?.cast_unsigned() {
        0x0000 => false,
        0x4000 => true,
        _ => return Err("numeric is NaN or infinite".into()),
    };
    let mut whole = Some(0i64);
    let mut real = 0f64;
    for i in 0..i32::from(ndigits) {
        let d = word(4 + usize::try_from(i)?)?;
        real += f64::from(d) * 10_000f64.powi(weight - i);
        if i <= weight {
            whole = whole.and_then(|n| n.checked_mul(10_000)?.checked_add(i64::from(d)));
        } else if d != 0 {
            whole = None;
        }
    }
    for _ in i32::from(ndigits)..=weight {
        whole = whole.and_then(|n| n.checked_mul(10_000));
    }
    Ok(match (whole, negative) {
        (Some(n), false) => Value::Integer(n),
        (Some(n), true) => Value::Integer(-n),
        (None, false) => Value::Real(real),
        (None, true) => Value::Real(-real),
    })
}

pub(super) mod target;
#[cfg(test)]
mod tests;
