//! Schema creation and stored-value parsing helpers.

use std::fs::{self, File, TryLockError};
use std::path::Path;

use moor_core::box_key::ControllerId;
use moor_core::cache::CacheSalt;
use moor_migrate::{Backend, Legacy, Migration};
use moor_schema::STEPS;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::open::sibling;
use super::{Store, StoreError};
use crate::env::CONTROLLER_LOCK;

impl Store {
    /// This state dir's cache key salt, made on first use and never changed.
    pub(crate) fn cache_salt(&self) -> Result<CacheSalt, StoreError> {
        let mut fresh = [0u8; 64];
        getrandom::fill(&mut fresh)
            .map_err(|e| StoreError::Corrupt(format!("no randomness for the cache salt: {e}")))?;
        let stored: String = {
            let conn = self.writer();
            conn.execute(
                "INSERT INTO meta (k, v) VALUES ('cache_salt', ?) ON CONFLICT (k) DO NOTHING",
                [CacheSalt::new(fresh).to_hex()],
            )?;
            conn.query_row("SELECT v FROM meta WHERE k = 'cache_salt'", [], |r| {
                r.get(0)
            })?
        };
        parsed_opt("meta cache_salt", &stored, CacheSalt::from_hex)
    }
}

impl Store {
    /// This store's controller id, made on first use and never changed: Box ids restart with a
    /// fresh store, so the id lives and dies with the database that numbers them.
    pub(crate) fn controller_id(&self) -> Result<ControllerId, StoreError> {
        let mut fresh = [0u8; 4];
        getrandom::fill(&mut fresh).map_err(|e| {
            StoreError::Corrupt(format!("no randomness for the controller id: {e}"))
        })?;
        let stored: String = {
            let conn = self.writer();
            conn.execute(
                "INSERT INTO meta (k, v) VALUES ('controller_id', ?) ON CONFLICT (k) DO NOTHING",
                [ControllerId::from_bytes(fresh).as_str()],
            )?;
            conn.query_row("SELECT v FROM meta WHERE k = 'controller_id'", [], |r| {
                r.get(0)
            })?
        };
        parsed_opt("meta controller_id", &stored, ControllerId::parse)
    }
}

/// moor.db's migrations, oldest first.
pub(super) fn schema() -> Result<Vec<Migration>, StoreError> {
    moor_migrate::steps(STEPS, Backend::Sqlite)
        .map_err(|e| StoreError::Corrupt(format!("migrations: {e}")))
}

/// Refuse an existing db this build would not migrate, without writing to it or its directory.
///
/// A db with no `-wal` file is read as immutable, because a read-only open of a WAL-mode db
/// creates the `-wal` and `-shm` files.
pub(super) fn refuse_other_version(db: &Path) -> Result<(), StoreError> {
    if !db.try_exists()? {
        return Ok(());
    }
    let escaped = db
        .to_string_lossy()
        .replace('%', "%25")
        .replace('?', "%3f")
        .replace('#', "%23");
    let immutable = if sibling(db, "-wal").try_exists()? {
        ""
    } else {
        "&immutable=1"
    };
    let conn = Connection::open_with_flags(
        format!("file:{escaped}?mode=ro{immutable}"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    moor_migrate::check(&conn, &schema()?, legacy).map_err(|e| refused(db, e))
}

/// Bring the db to the latest migration; one process migrates while others wait.
pub(super) fn migrate(conn: &mut Connection, db: &Path) -> Result<(), StoreError> {
    moor_migrate::migrate(conn, &schema()?, legacy).map_err(|e| refused(db, e))
}

/// The state dir's controller lock when `db` needs migrating, to hold while it migrates, so a
/// running controller's db never moves past the schema that controller and its Proxy know.
pub(super) fn claim_migration(conn: &Connection, db: &Path) -> Result<Option<File>, StoreError> {
    if moor_migrate::is_latest(conn, &schema()?).map_err(|e| refused(db, e))? {
        return Ok(None);
    }
    owner_lock(db)
}

/// The controller lock beside `db`, held until dropped; none when this process is the
/// controller holding it. Another holder is a refusal naming its pid and socket.
fn owner_lock(db: &Path) -> Result<Option<File>, StoreError> {
    let path = db.with_file_name(CONTROLLER_LOCK);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => {
            let held = fs::read_to_string(&path).unwrap_or_default();
            let mut lines = held.lines();
            let pid = lines.next().unwrap_or("?").to_owned();
            if pid == std::process::id().to_string() {
                return Ok(None);
            }
            Err(StoreError::Owned {
                db: db.to_path_buf(),
                pid,
                socket: lines.next().unwrap_or("?").to_owned(),
            })
        }
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// What a db without a migration history holds: nothing, or a pre-migration store named by its
/// `meta` schema version, which this moor does not adopt.
fn legacy(conn: &Connection) -> rusqlite::Result<Legacy> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |r| r.get(0),
    )?;
    if !has_meta {
        return Ok(Legacy::Empty);
    }
    let sql = "SELECT v FROM meta WHERE k = 'schema_version'";
    let found: Option<String> = conn.query_row(sql, [], |r| r.get(0)).optional()?;
    Ok(Legacy::Other(found.unwrap_or_else(|| "none".to_owned())))
}

/// The store error for a db this build will not migrate.
fn refused(db: &Path, e: moor_migrate::Error) -> StoreError {
    match e {
        moor_migrate::Error::Unadoptable(version) => StoreError::Schema {
            db: db.to_path_buf(),
            found: format!("schema version `{version}`"),
        },
        moor_migrate::Error::Squashed { found, .. } => StoreError::Schema {
            db: db.to_path_buf(),
            found: format!("migration {found}"),
        },
        moor_migrate::Error::Sqlite(e) => StoreError::Sqlite(e),
        other => StoreError::Migration {
            db: db.to_path_buf(),
            source: other,
        },
    }
}

/// Parse a stored text value, naming the column on failure.
pub(super) fn parsed<T, E: std::fmt::Display>(
    what: &str,
    raw: &str,
    f: impl FnOnce(&str) -> Result<T, E>,
) -> Result<T, StoreError> {
    f(raw).map_err(|e| StoreError::Corrupt(format!("{what} `{raw}`: {e}")))
}

/// Parse a stored enum value that has an `Option`-returning parser.
pub(super) fn parsed_opt<T>(
    what: &str,
    raw: &str,
    f: impl FnOnce(&str) -> Option<T>,
) -> Result<T, StoreError> {
    parsed(what, raw, |s| f(s).ok_or("unknown value"))
}

#[cfg(test)]
mod tests;
