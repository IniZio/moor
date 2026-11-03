//! Where a store's connections open: a `SQLite` file, or a Postgres database.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use moor_core::request::Pins;
use moor_core::seam::clock::Clock;

use super::db::{self, Conn};
use super::{init, Store, StoreError, BUSY_TIMEOUT};

/// Where a store's read connections open.
#[derive(Clone)]
pub(super) enum Source {
    /// The `SQLite` file.
    Sqlite(PathBuf),
    /// The Postgres database.
    Postgres(Arc<postgres::Config>),
}

impl Store {
    /// Open the store in the Postgres database `config` names, bringing its schema to the latest
    /// migration first. `state_dir` holds the files only this host uses.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the cluster opens it once its config names a database"
        )
    )]
    pub(crate) fn open_postgres(
        config: postgres::Config,
        state_dir: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StoreError> {
        let mut client = config.connect(postgres::NoTls).map_err(db::Error::from)?;
        init::migrate_postgres(&mut client)?;
        Ok(Self {
            source: Source::Postgres(Arc::new(config)),
            runs: state_dir.join("runs"),
            writer: Arc::new(Mutex::new(Conn::Postgres(Box::new(db::PgConn::new(
                client,
            ))))),
            clock,
            pins: Pins::new(),
        })
    }

    /// A new connection for reads, read-only on `SQLite`; never blocks on the writer.
    pub(crate) fn reader(&self) -> Result<Conn, StoreError> {
        Ok(match &self.source {
            Source::Sqlite(path) => Conn::sqlite_reader(path, BUSY_TIMEOUT)?,
            Source::Postgres(config) => Conn::Postgres(Box::new(db::PgConn::new(
                config.connect(postgres::NoTls).map_err(db::Error::from)?,
            ))),
        })
    }
}
