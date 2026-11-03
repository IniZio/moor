//! Where a store's connections open: a `SQLite` file, or a Postgres database.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::request::Pins;
use moor_core::seam::clock::Clock;

use super::db::{self, Backend, Conn, Db, PgConn, Rows, Target};
use super::{init, Store, StoreError, BUSY_TIMEOUT, STORE_FILE};
use rusqlite::types::Value;

/// The most Postgres read connections kept open between reads.
const IDLE_READERS: usize = 8;

/// Where a store's read connections open.
#[derive(Clone)]
pub(super) enum Source {
    /// The `SQLite` file.
    Sqlite(PathBuf),
    /// The Postgres database.
    Postgres(Arc<Readers>),
}

/// Postgres connections for reads, each read-only, kept open between reads.
pub(super) struct Readers {
    /// Where they connect, read-only.
    target: Arc<Target>,
    /// The ones no read holds now.
    idle: Mutex<Vec<PgConn>>,
}

impl Readers {
    /// An idle connection, else a new one.
    fn take(&self) -> Result<PgConn, db::Error> {
        let idle = self
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        idle.map_or_else(|| PgConn::connect(Arc::clone(&self.target)), Ok)
    }

    /// Keep `conn` for the next read. One that closed, as when the server restarted, is dropped
    /// with every idle one, which closed with it, so only the read that found it fails.
    fn give_back(&self, conn: PgConn) {
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        if conn.is_closed() {
            idle.clear();
        } else if idle.len() < IDLE_READERS {
            idle.push(conn);
        }
    }
}

/// A connection for reads, read-only on both backends.
pub(crate) struct Reader<'s> {
    /// The connection; taken only when the reader drops.
    conn: Option<Conn>,
    /// Where a Postgres connection goes back to.
    readers: Option<&'s Readers>,
}

impl Reader<'_> {
    /// The connection.
    fn conn(&self) -> &Conn {
        self.conn
            .as_ref()
            .unwrap_or_else(|| unreachable!("taken only on drop"))
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        if let (Some(readers), Some(Conn::Postgres(conn))) = (self.readers, self.conn.take()) {
            readers.give_back(*conn);
        }
    }
}

impl Db for Reader<'_> {
    fn execute_values(&self, sql: &str, params: Vec<Value>) -> db::Result<usize> {
        self.conn().execute_values(sql, params)
    }

    fn query_values(&self, sql: &str, params: Vec<Value>) -> db::Result<Rows> {
        self.conn().query_values(sql, params)
    }

    fn backend(&self) -> Backend {
        self.conn().backend()
    }
}

impl Store {
    /// Open the store in the Postgres database `url` names, else in `moor.db` in `state_dir`.
    pub(crate) fn open_in(
        state_dir: &Path,
        url: Option<&str>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StoreError> {
        match url {
            None => Self::open(&state_dir.join(STORE_FILE), clock),
            Some(url) => {
                let target = Target::parse(url).map_err(StoreError::Url)?;
                Self::open_postgres(target, state_dir, clock)
            }
        }
    }

    /// Open the store in the Postgres database `target`, bringing its schema to the latest
    /// migration first. `state_dir` holds the files only this host uses.
    pub(crate) fn open_postgres(
        target: Target,
        state_dir: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StoreError> {
        let at = target.shown().to_owned();
        let unreachable = |source: db::Error| StoreError::Unreachable { at, source };
        let readers = Arc::new(target.read_only());
        let mut client = match target.connect() {
            Ok(client) => client,
            Err(e) => return Err(unreachable(e.into())),
        };
        init::migrate_postgres(&mut client)?;
        drop(client);
        let writer = PgConn::connect(Arc::new(target)).map_err(unreachable)?;
        Ok(Self {
            source: Source::Postgres(Arc::new(Readers {
                target: readers,
                idle: Mutex::default(),
            })),
            runs: state_dir.join("runs"),
            writer: Arc::new(Mutex::new(Conn::Postgres(Box::new(writer)))),
            clock,
            pins: Pins::new(),
        })
    }

    /// A connection for reads, read-only; never blocks on the writer.
    pub(crate) fn reader(&self) -> Result<Reader<'_>, StoreError> {
        debug_assert_off_runtime();
        Ok(match &self.source {
            Source::Sqlite(path) => Reader {
                conn: Some(Conn::sqlite_reader(path, BUSY_TIMEOUT)?),
                readers: None,
            },
            Source::Postgres(readers) => Reader {
                conn: Some(Conn::Postgres(Box::new(readers.take()?))),
                readers: Some(readers),
            },
        })
    }

    /// Where the store lives, without a password: the Postgres URL or the `SQLite` file.
    pub(crate) fn location(&self) -> String {
        match &self.source {
            Source::Sqlite(path) => path.display().to_string(),
            Source::Postgres(readers) => readers.target.shown().to_owned(),
        }
    }

    /// The Postgres URL this store opened, password included, for a child process that opens
    /// the same store; none on `SQLite`.
    pub(crate) fn database_url(&self) -> Option<&str> {
        match &self.source {
            Source::Sqlite(_) => None,
            Source::Postgres(readers) => Some(readers.target.url()),
        }
    }
}

/// In debug builds, panic when called on a tokio runtime thread. A store call blocks, and the
/// Postgres client panics there, so this catches such a call in tests on `SQLite` too.
pub(super) fn debug_assert_off_runtime() {
    debug_assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "a store call ran on a tokio runtime thread"
    );
}
