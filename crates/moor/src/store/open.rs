//! Opening the store on its database: a `SQLite` file, or a Postgres database.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::request::Pins;
use moor_core::seam::clock::Clock;
use moor_migrate::Outcome;

use super::db::{self, Opened, Target};
use super::{init, Store, StoreError, BUSY_TIMEOUT, STORE_FILE};
use crate::env::Env;

/// The Postgres database a store lives in.
#[derive(Debug, Clone)]
pub(crate) struct Database {
    /// Its `postgres://` URL, password included.
    pub(crate) url: String,
    /// The most read connections the store opens to it.
    pub(crate) readers: usize,
}

/// The read connections a store opens when `MOOR_DATABASE_READERS` is unset.
const DEFAULT_READERS: usize = 4;

impl Database {
    /// The database `env` names, when `MOOR_DATABASE_URL` is set, with the cap
    /// `MOOR_DATABASE_READERS` puts on its read connections.
    pub(crate) fn from_env(env: &Env) -> Result<Option<Self>, StoreError> {
        let Some(url) = env.database_url() else {
            return Ok(None);
        };
        let readers = match env.database_readers() {
            None => DEFAULT_READERS,
            Some(n) => n.parse::<usize>().ok().filter(|n| *n >= 1).ok_or_else(|| {
                StoreError::Readers(format!("`{n}` is not a whole number of 1 or more"))
            })?,
        };
        Ok(Some(Self {
            url: url.to_owned(),
            readers,
        }))
    }
}

impl Store {
    /// Open (creating if needed) the store at `path`: state dir 0700, db, `-wal` and `-shm` 0600.
    /// A db this build would not migrate is refused before anything writes to it or changes a mode.
    /// A db a newer build migrated opens unchanged.
    pub(crate) fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self, StoreError> {
        let opened = db::open_sqlite(path, BUSY_TIMEOUT, init::owner_lock)?;
        Ok(Self::with(opened, path.with_file_name("runs"), clock))
    }

    /// Open the store in the Postgres database `url` names, else in `moor.db` in `state_dir`,
    /// for a moor that is not the controller. It takes the controller lock for as long as the
    /// store lives, and is refused if a controller holds it, as a Box lock or run claim here
    /// would not reach that controller's. Another such moor holding it past [`init::LOCK_WAIT`]
    /// is a refusal too.
    pub(crate) fn open_in(
        state_dir: &Path,
        database: Option<&Database>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StoreError> {
        Self::open_in_within(state_dir, database, clock, init::LOCK_WAIT)
    }

    /// As [`Store::open_in`], waiting up to `wait` for a state dir in a file.
    pub(crate) fn open_in_within(
        state_dir: &Path,
        database: Option<&Database>,
        clock: Arc<dyn Clock>,
        wait: Duration,
    ) -> Result<Self, StoreError> {
        let mut excludes = None;
        let mut state_lock = None;
        match database {
            Some(database) => {
                let target = Target::parse(&database.url).map_err(StoreError::Url)?;
                let lock = db::ControllerLock::exclude(&target, "moor command")?;
                excludes = Some(Arc::new(Mutex::new(lock)));
            }
            None => state_lock = init::exclude(state_dir, wait)?.map(Arc::new),
        }
        Ok(Self {
            _excludes: excludes,
            _state_lock: state_lock,
            ..Self::open_claimed(state_dir, database, clock)?
        })
    }

    /// As [`Store::open_in`], for the controller, which already holds the right to run here.
    pub(crate) fn open_claimed(
        state_dir: &Path,
        database: Option<&Database>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StoreError> {
        match database {
            None => Self::open(&state_dir.join(STORE_FILE), clock),
            Some(database) => {
                let target = Target::parse(&database.url).map_err(StoreError::Url)?;
                Self::open_postgres(&target, database.readers, state_dir, clock)
            }
        }
    }

    /// Open the store in the Postgres database `target`, bringing its schema to this build's
    /// migrations first, opening at most `readers` read connections to it. A database a newer
    /// build migrated opens unchanged. `state_dir` holds the files only this host uses.
    pub(crate) fn open_postgres(
        target: &Target,
        readers: usize,
        state_dir: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StoreError> {
        let opened = db::open_postgres(target, readers)?;
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
            _excludes: None,
            _state_lock: None,
            #[cfg(test)]
            write_check: Arc::default(),
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
}

#[cfg(test)]
mod tests;
