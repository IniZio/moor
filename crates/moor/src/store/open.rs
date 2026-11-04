//! Opening the store on its database: a `SQLite` file, or a Postgres database.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use moor_core::request::Pins;
use moor_core::seam::clock::Clock;
use moor_migrate::Outcome;

use super::db::{self, Opened, Target};
use super::{init, Store, StoreError, BUSY_TIMEOUT, STORE_FILE};

impl Store {
    /// Open (creating if needed) the store at `path`: state dir 0700, db, `-wal` and `-shm` 0600.
    /// A db this build would not migrate is refused before anything writes to it or changes a mode.
    /// A db a newer build migrated opens unchanged.
    pub(crate) fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self, StoreError> {
        let opened = db::open_sqlite(path, BUSY_TIMEOUT, init::owner_lock)?;
        Ok(Self::with(opened, path.with_file_name("runs"), clock))
    }

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
                Self::open_postgres(&target, state_dir, clock)
            }
        }
    }

    /// Open the store in the Postgres database `target`, bringing its schema to this build's
    /// migrations first. A database a newer build migrated opens unchanged. `state_dir` holds
    /// the files only this host uses.
    pub(crate) fn open_postgres(
        target: &Target,
        state_dir: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StoreError> {
        let opened = db::open_postgres(target)?;
        Ok(Self::with(opened, state_dir.join("runs"), clock))
    }

    /// A store on the database just `opened`, with its follow claims in `runs`.
    fn with(opened: Opened, runs: PathBuf, clock: Arc<dyn Clock>) -> Self {
        let Opened {
            writer,
            readers,
            schema,
        } = opened;
        Self {
            readers: Arc::new(readers),
            runs,
            writer: Arc::new(Mutex::new(writer)),
            clock,
            pins: Pins::new(),
            schema,
        }
    }

    /// The db's standing against this build's migrations; a db a newer build migrated is opened
    /// as found, and the caller says so.
    pub(crate) const fn schema(&self) -> Outcome {
        self.schema
    }

    /// Where the store lives, without a password: the `SQLite` file or the Postgres URL.
    pub(crate) fn location(&self) -> String {
        self.readers.location()
    }

    /// The Postgres URL this store opened, password included, for a child process that opens
    /// the same store; none on `SQLite`.
    pub(crate) fn database_url(&self) -> Option<&str> {
        self.readers.database_url()
    }
}
