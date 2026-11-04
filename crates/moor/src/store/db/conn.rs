//! Connections and transactions on either backend, and how the store opens its database.

use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_migrate::Outcome;

use super::{postgres, sqlite, Db, Result, Rows, Statement, Target};
use crate::store::StoreError;

/// An open connection to the store's database.
pub(crate) enum Conn {
    /// A `SQLite` file.
    Sqlite(sqlite::Conn),
    /// A Postgres database.
    Postgres(Box<postgres::Conn>),
}

/// An open transaction; dropping it rolls back.
pub(crate) enum Tx<'c> {
    /// On `SQLite`.
    Sqlite(sqlite::Tx<'c>),
    /// On Postgres.
    Postgres(postgres::Tx<'c>),
}

/// A store's database, just opened: its writer, where its readers open, and its standing
/// against this build's migrations.
pub(crate) struct Opened {
    /// The one connection that writes.
    pub(crate) writer: Conn,
    /// Where read connections open.
    pub(crate) readers: Readers,
    /// Whether a newer build migrated the database past this one.
    pub(crate) schema: Outcome,
}

/// Open (creating if needed) the store's `SQLite` db at `path`, waiting `busy` on a lock, and
/// migrated to this build's schema while holding what `claim` returns. A db this build would not
/// migrate is refused before anything writes to it; one a newer build migrated opens unchanged.
pub(crate) fn open_sqlite(
    path: &Path,
    busy: Duration,
    claim: impl FnOnce(&Path) -> Result<Option<std::fs::File>, StoreError>,
) -> Result<Opened, StoreError> {
    let (conn, schema) = sqlite::Conn::writer(path, busy, claim)?;
    Ok(Opened {
        writer: Conn::Sqlite(conn),
        readers: Readers::Sqlite {
            path: path.to_path_buf(),
            busy,
        },
        schema,
    })
}

/// Open the store's Postgres database `target`, migrated to this build's schema; one a newer
/// build migrated opens unchanged.
pub(crate) fn open_postgres(target: &Target) -> Result<Opened, StoreError> {
    let (conn, schema) = postgres::Conn::writer(target)?;
    Ok(Opened {
        writer: Conn::Postgres(Box::new(conn)),
        readers: Readers::Postgres(Box::new(postgres::Pool::new(target.read_only()))),
        schema,
    })
}

impl Conn {
    /// A plain read-write connection to the db file at `path`, as a test outside the store
    /// opens one beside a live store.
    #[cfg(test)]
    pub(crate) fn open_file(path: &Path) -> Result<Self> {
        sqlite::Conn::open_file(path).map(Self::Sqlite)
    }

    /// Run several `;`-separated statements that bind nothing.
    #[cfg(test)]
    pub(crate) fn execute_batch(&self, sql: &str) -> Result<()> {
        match self {
            Self::Sqlite(c) => c.execute_batch(sql),
            Self::Postgres(c) => c.execute_batch(sql),
        }
    }

    /// Run the schema statement `stmt`, rendered for this connection's database.
    #[cfg(test)]
    pub(crate) fn schema(&self, stmt: &impl sea_query::SchemaStatementBuilder) -> Result<()> {
        match self {
            Self::Sqlite(c) => c.execute_batch(&stmt.to_string(sea_query::SqliteQueryBuilder)),
            Self::Postgres(c) => c.execute_batch(&stmt.to_string(sea_query::PostgresQueryBuilder)),
        }
    }

    /// Begin a transaction that takes the write lock now, so a read in it is not stale by the
    /// time it writes.
    pub(crate) fn immediate(&mut self) -> Result<Tx<'_>> {
        Ok(match self {
            Self::Sqlite(c) => Tx::Sqlite(c.immediate()?),
            Self::Postgres(c) => Tx::Postgres(c.transaction()?),
        })
    }

    /// Begin a transaction that takes the write lock at its first write.
    pub(crate) fn transaction(&mut self) -> Result<Tx<'_>> {
        Ok(match self {
            Self::Sqlite(c) => Tx::Sqlite(c.transaction()?),
            Self::Postgres(c) => Tx::Postgres(c.transaction()?),
        })
    }
}

impl Tx<'_> {
    /// Commit it.
    pub(crate) fn commit(self) -> Result<()> {
        match self {
            Self::Sqlite(tx) => tx.commit(),
            Self::Postgres(tx) => tx.commit(),
        }
    }
}

impl Db for Conn {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        match self {
            Self::Sqlite(c) => c.run_statement(s),
            Self::Postgres(c) => c.run_statement(s),
        }
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        match self {
            Self::Sqlite(c) => c.fetch_statement(s),
            Self::Postgres(c) => c.fetch_statement(s),
        }
    }
}

impl Db for Tx<'_> {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        match self {
            Self::Sqlite(tx) => tx.run_statement(s),
            Self::Postgres(tx) => tx.run_statement(s),
        }
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        match self {
            Self::Sqlite(tx) => tx.fetch_statement(s),
            Self::Postgres(tx) => tx.fetch_statement(s),
        }
    }
}

impl Db for std::sync::MutexGuard<'_, Conn> {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        (**self).run_statement(s)
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        (**self).fetch_statement(s)
    }
}

/// Where the store's read connections open.
pub(crate) enum Readers {
    /// Read-only connections to the `SQLite` file at `path`, waiting `busy` on a lock.
    Sqlite {
        /// The db file.
        path: PathBuf,
        /// How long a read waits on a lock.
        busy: Duration,
    },
    /// Read-only Postgres connections, kept open between reads.
    Postgres(Box<postgres::Pool>),
}

impl Readers {
    /// Where the store lives, without a password: the `SQLite` file or the Postgres URL.
    pub(crate) fn location(&self) -> String {
        match self {
            Self::Sqlite { path, .. } => path.display().to_string(),
            Self::Postgres(pool) => pool.target().shown().to_owned(),
        }
    }

    /// The Postgres URL, password included, for a child process that opens the same store;
    /// none on `SQLite`.
    pub(crate) fn database_url(&self) -> Option<&str> {
        match self {
            Self::Sqlite { .. } => None,
            Self::Postgres(pool) => Some(pool.target().url()),
        }
    }

    /// A connection for reads; it never blocks on the writer.
    pub(crate) fn open(&self) -> Result<Reader<'_>> {
        Ok(match self {
            Self::Sqlite { path, busy } => Reader::Sqlite(sqlite::Conn::reader(path, *busy)?),
            Self::Postgres(pool) => Reader::Postgres(Box::new(pool.take()?)),
        })
    }
}

/// A read-only connection; a Postgres one goes back to its pool when dropped.
pub(crate) enum Reader<'p> {
    /// On `SQLite`.
    Sqlite(sqlite::Conn),
    /// On Postgres.
    Postgres(Box<postgres::Pooled<'p>>),
}

impl Db for Reader<'_> {
    fn run_statement(&self, s: &Statement) -> Result<usize> {
        match self {
            Self::Sqlite(c) => c.run_statement(s),
            Self::Postgres(c) => c.run_statement(s),
        }
    }

    fn fetch_statement(&self, s: &Statement) -> Result<Rows> {
        match self {
            Self::Sqlite(c) => c.fetch_statement(s),
            Self::Postgres(c) => c.fetch_statement(s),
        }
    }
}

/// The store error for a db at `at` this build will not migrate.
pub(super) fn refused(at: &str, e: moor_migrate::Error) -> StoreError {
    match e {
        moor_migrate::Error::Unadoptable(version) => StoreError::Schema {
            db: at.to_owned(),
            found: format!("schema version `{version}`"),
        },
        moor_migrate::Error::Squashed { found, .. } => StoreError::Schema {
            db: at.to_owned(),
            found: format!("migration {found}"),
        },
        moor_migrate::Error::Sqlite(e) => StoreError::Db(sqlite::error(e)),
        moor_migrate::Error::Postgres(e) => StoreError::Db(postgres::error(e)),
        other @ moor_migrate::Error::Migration(_) => StoreError::Migration {
            db: at.to_owned(),
            source: other,
        },
    }
}
