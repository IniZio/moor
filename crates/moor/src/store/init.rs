//! Schema creation and stored-value parsing helpers.

use std::path::Path;

use moor_core::cache::CacheSalt;
use moor_migrate::{Legacy, Migration};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::open::sibling;
use super::{Store, StoreError};

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

/// The pre-migration schema version that migration 1 reproduces; a file at it is adopted.
pub(super) const BASELINE_VERSION: &str = "16";

/// moor.db's migrations, oldest first. Never edit an applied one; add a new file.
pub(super) fn schema() -> Result<Vec<Migration>, StoreError> {
    moor_migrate::migrations(&[
        (
            "V1__baseline.sql",
            include_str!("../../migrations/V1__baseline.sql"),
        ),
        (
            "V2__tables_made_on_open.sql",
            include_str!("../../migrations/V2__tables_made_on_open.sql"),
        ),
        (
            "V3__user_mcp_servers.sql",
            include_str!("../../migrations/V3__user_mcp_servers.sql"),
        ),
        (
            "V4__binding_history.sql",
            include_str!("../../migrations/V4__binding_history.sql"),
        ),
        (
            "V5__user_machines.sql",
            include_str!("../../migrations/V5__user_machines.sql"),
        ),
        (
            "V6__box_machines.sql",
            include_str!("../../migrations/V6__box_machines.sql"),
        ),
        (
            "V7__machine_slots.sql",
            include_str!("../../migrations/V7__machine_slots.sql"),
        ),
        (
            "V8__machine_sync_exclude.sql",
            include_str!("../../migrations/V8__machine_sync_exclude.sql"),
        ),
    ])
    .map_err(|e| StoreError::Corrupt(format!("migration files: {e}")))
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

/// What a db without a migration history holds, read from its `meta` schema version.
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
    Ok(match found {
        Some(v) if v == BASELINE_VERSION => Legacy::Baseline,
        other => Legacy::Other(other.unwrap_or_else(|| "none".to_owned())),
    })
}

/// The store error for a db this build will not migrate.
fn refused(db: &Path, e: moor_migrate::Error) -> StoreError {
    match e {
        moor_migrate::Error::Unadoptable(found) => StoreError::Schema {
            db: db.to_path_buf(),
            found,
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
