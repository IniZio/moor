//! The Postgres backend: statements rendered with `sea-query`'s Postgres builder, each value
//! bound as its storage class's Postgres type, each column read back as a `SQLite` storage
//! class, and a store-wide advisory lock standing in for `SQLite`'s write lock.

use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use moor_migrate::{Backend, Outcome};
use moor_schema::STEPS;
use postgres::error::{DbError, SqlState};
use postgres::types::{ToSql, Type};
use postgres::{Client, GenericClient, Transaction};
use sea_query::PostgresQueryBuilder;

use super::{Db, Error, Result, Rows, Statement, UniqueKey, Value};
use crate::store::StoreError;
use types::{Bind, Read};

pub(crate) use target::Target;

/// Taken by every transaction, keyed by schema so stores in separate schemas of one database
/// never wait on each other. A `SQLite` transaction that writes holds the file's one write lock,
/// and the store relies on that, so a Postgres one holds this from its start.
const WRITE_LOCK: &str = "SELECT pg_advisory_xact_lock(hashtext(current_schema()), 1)";

/// The most statements a connection keeps prepared before it forgets them all.
const PREPARED: usize = 512;

/// The most read connections a pool keeps open between reads.
const IDLE_READERS: usize = 8;

/// A connection's prepared statements, by their SQL and parameter types.
type Prepared = RefCell<HashMap<String, postgres::Statement>>;

/// An open connection to the store's Postgres database. A call on a connection that closed, as
/// when the server restarted, connects again first; the call that found it closed failed and is
/// not run again.
pub(crate) struct Conn {
    /// Where it connects.
    target: Target,
    /// The connection.
    client: RefCell<Client>,
    /// Statements prepared on it.
    prepared: Prepared,
}

/// An open transaction; dropping it rolls back.
pub(crate) struct Tx<'c> {
    /// The transaction.
    tx: RefCell<Transaction<'c>>,
    /// Its connection's prepared statements.
    prepared: &'c Prepared,
}

impl Conn {
    /// A connection to `target`.
    pub(super) fn connect(target: &Target) -> Result<Self> {
        Ok(Self {
            client: RefCell::new(target.connect().map_err(error)?),
            target: target.clone(),
            prepared: RefCell::default(),
        })
    }

    /// The connection, made again first when it has closed.
    fn client(&self) -> Result<RefMut<'_, Client>> {
        if self.client.borrow().is_closed() {
            *self.client.borrow_mut() = self.target.connect().map_err(error)?;
            self.prepared.borrow_mut().clear();
        }
        Ok(self.client.borrow_mut())
    }

    /// The store's writer on `target`, its schema migrated to this build's first. A database a
    /// newer build migrated is left as found, and the outcome says so.
    pub(super) fn writer(target: &Target) -> Result<(Self, Outcome), StoreError> {
        let conn = Self::connect(target).map_err(|source| StoreError::Unreachable {
            at: target.shown().to_owned(),
            source,
        })?;
        let set = moor_migrate::steps(STEPS, Backend::Postgres)
            .map_err(|e| StoreError::Corrupt(format!("migrations: {e}")))?;
        let outcome = moor_migrate::migrate_postgres(&mut conn.client.borrow_mut(), &set)
            .map_err(|e| super::conn::refused(target.shown(), e))?;
        Ok((conn, outcome))
    }

    /// Begin a transaction holding the store's write lock.
    pub(super) fn transaction(&mut self) -> Result<Tx<'_>> {
        drop(self.client()?);
        let Self {
            client, prepared, ..
        } = self;
        let mut tx = client.get_mut().transaction().map_err(error)?;
        tx.batch_execute(WRITE_LOCK).map_err(error)?;
        Ok(Tx {
            tx: RefCell::new(tx),
            prepared,
        })
    }

    /// Run several `;`-separated statements that bind nothing.
    #[cfg(test)]
    pub(super) fn execute_batch(&self, sql: &str) -> Result<()> {
        self.client()?.batch_execute(sql).map_err(error)
    }
}

impl Tx<'_> {
    /// Commit it.
    pub(super) fn commit(self) -> Result<()> {
        self.tx.into_inner().commit().map_err(error)
    }

    /// Make `column` of `table` assign ids past the largest it holds, when a sequence assigns
    /// them (a serial or identity column); any other column is left alone. A table with no rows
    /// keeps its sequence where it is.
    pub(super) fn continue_ids(&self, table: &str, column: &str) -> Result<()> {
        let mut tx = self.tx.borrow_mut();
        let sequence: Option<String> = tx
            .query_one("SELECT pg_get_serial_sequence($1, $2)", &[&table, &column])
            .map_err(error)?
            .try_get(0)
            .map_err(error)?;
        let Some(sequence) = sequence else {
            return Ok(());
        };
        let quote = |name: &str| name.replace('"', "\"\"");
        tx.execute(
            &format!(
                "SELECT setval($1::text::regclass, m) \
                 FROM (SELECT MAX(\"{}\") AS m FROM \"{}\") peak WHERE m IS NOT NULL",
                quote(column),
                quote(table),
            ),
            &[&sequence],
        )
        .map_err(error)?;
        Ok(())
    }
}

impl Db for Conn {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        execute(&mut *self.client()?, &self.prepared, s)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        query(&mut *self.client()?, &self.prepared, s)
    }
}

impl Db for Tx<'_> {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        execute(&mut *self.tx.borrow_mut(), self.prepared, s)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        query(&mut *self.tx.borrow_mut(), self.prepared, s)
    }
}

/// Read-only connections to one database, kept open between reads.
pub(crate) struct Pool {
    /// Where they connect, read-only.
    target: Target,
    /// The ones no read holds now.
    idle: Mutex<Vec<Conn>>,
}

impl Pool {
    /// Where its connections connect.
    pub(super) const fn target(&self) -> &Target {
        &self.target
    }

    /// A pool of connections to `target`.
    pub(super) fn new(target: Target) -> Self {
        Self {
            target,
            idle: Mutex::default(),
        }
    }

    /// An idle connection, else a new one.
    pub(super) fn take(&self) -> Result<Pooled<'_>> {
        let idle = self
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let conn = match idle {
            Some(conn) => conn,
            None => Conn::connect(&self.target)?,
        };
        Ok(Pooled {
            conn: Some(conn),
            pool: self,
        })
    }

    /// Keep `conn` for the next read. One that closed, as when the server restarted, is dropped
    /// with every idle one, which closed with it.
    fn give_back(&self, conn: Conn) {
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        if conn.client.borrow().is_closed() {
            idle.clear();
        } else if idle.len() < IDLE_READERS {
            idle.push(conn);
        }
    }
}

/// A connection taken from a [`Pool`], given back when dropped.
pub(crate) struct Pooled<'p> {
    /// The connection; taken only when this drops.
    conn: Option<Conn>,
    /// Where it goes back to.
    pool: &'p Pool,
}

impl Pooled<'_> {
    /// The connection.
    fn conn(&self) -> Result<&Conn> {
        self.conn
            .as_ref()
            .ok_or_else(|| Error::Backend("the reader was given back".into()))
    }
}

impl Drop for Pooled<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.give_back(conn);
        }
    }
}

impl Db for Pooled<'_> {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        self.conn()?.run_statement(s)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        self.conn()?.fetch_statement(s)
    }
}

/// `s` rendered and prepared on `db`, from `cache` when it was prepared before, with the
/// values it binds. Each parameter is typed by its value's storage class, as a `SQLite` value
/// keeps its own, so a parameter whose type the statement does not fix still has one; a null
/// is left for Postgres to infer.
fn prepare(
    db: &mut impl GenericClient,
    cache: &Prepared,
    s: &Statement,
) -> Result<(postgres::Statement, Vec<Value>)> {
    let (sql, values) = s.render(PostgresQueryBuilder)?;
    let types: Vec<Type> = values.iter().map(param_type).collect();
    let key = types.iter().fold(sql.clone(), |mut key, ty| {
        key.push(' ');
        key.push_str(ty.name());
        key
    });
    if let Some(stmt) = cache.borrow().get(&key) {
        return Ok((stmt.clone(), values));
    }
    let stmt = db.prepare_typed(&sql, &types).map_err(error)?;
    let mut cache = cache.borrow_mut();
    if cache.len() >= PREPARED {
        cache.clear();
    }
    cache.insert(key, stmt.clone());
    Ok((stmt, values))
}

/// The parameter type `v` binds as.
const fn param_type(v: &Value) -> Type {
    match v {
        Value::Null => Type::UNKNOWN,
        Value::Integer(_) => Type::INT8,
        Value::Real(_) => Type::FLOAT8,
        Value::Text(_) => Type::TEXT,
        Value::Blob(_) => Type::BYTEA,
    }
}

/// Run `s` on `db`; the rows it changed.
fn execute(db: &mut impl GenericClient, cache: &Prepared, s: &Statement) -> Result<usize> {
    let (stmt, values) = prepare(db, cache, s)?;
    let binds: Vec<Bind<'_>> = values.iter().map(Bind).collect();
    let args: Vec<&(dyn ToSql + Sync)> = binds.iter().map(|b| b as _).collect();
    let n = db.execute(&stmt, &args).map_err(error)?;
    usize::try_from(n).map_err(|e| Error::Backend(Box::new(e)))
}

/// Query `s` on `db`, reading every row as `SQLite` values.
fn query(db: &mut impl GenericClient, cache: &Prepared, s: &Statement) -> Result<Rows> {
    let (stmt, values) = prepare(db, cache, s)?;
    let binds: Vec<Bind<'_>> = values.iter().map(Bind).collect();
    let args: Vec<&(dyn ToSql + Sync)> = binds.iter().map(|b| b as _).collect();
    let names = stmt.columns().iter().map(|c| c.name().to_owned()).collect();
    let rows = db
        .query(&stmt, &args)
        .map_err(error)?
        .iter()
        .map(|row| {
            (0..row.len())
                .map(|i| row.try_get::<_, Read>(i).map(|r| r.0).map_err(error))
                .collect::<Result<_>>()
        })
        .collect::<Result<_>>()?;
    Ok(Rows { names, rows })
}

/// `e` as the seam's error: a UNIQUE violation names its key.
#[expect(
    clippy::needless_pass_by_value,
    reason = "taken by value to fit `map_err`"
)]
pub(super) fn error(e: postgres::Error) -> Error {
    let Some(db) = e.as_db_error() else {
        let message = std::error::Error::source(&e)
            .map_or_else(|| e.to_string(), |cause| format!("{e}: {cause}"));
        return Error::Backend(message.into());
    };
    unique_key(db).map_or_else(
        || Error::Backend(db.to_string().into()),
        |key| Error::Unique {
            key,
            message: db.to_string(),
        },
    )
}

/// The key a UNIQUE violation names: its table, and its columns as the detail lists them in
/// `Key (a, b)=(..) already exists.`
fn unique_key(db: &DbError) -> Option<UniqueKey> {
    if *db.code() != SqlState::UNIQUE_VIOLATION {
        return None;
    }
    let (columns, _) = db.detail()?.strip_prefix("Key (")?.split_once(")=(")?;
    Some(UniqueKey {
        table: db.table()?.to_owned(),
        columns: columns
            .split(", ")
            .map(|c| c.trim_matches('"').to_owned())
            .collect(),
    })
}

mod target;
#[cfg(test)]
mod tests;
mod types;
