//! Bringing a `SQLite` db file to this build's migrations, and refusing one it would not migrate.

use std::path::Path;

use moor_migrate::{Backend, Legacy, Migration, Outcome};
use moor_schema::STEPS;
use rusqlite::{Connection, OpenFlags, OptionalExtension as _};

use super::error;
use super::files::sibling;
use crate::store::StoreError;

/// The store error for the db at `db` this build will not migrate.
fn refused(db: &Path, e: moor_migrate::Error) -> StoreError {
    crate::store::db::conn::refused(&db.display().to_string(), e)
}

/// moor.db's migrations, oldest first.
fn schema() -> Result<Vec<Migration>, StoreError> {
    moor_migrate::steps(STEPS, Backend::Sqlite)
        .map_err(|e| StoreError::Corrupt(format!("migrations: {e}")))
}

/// Refuse an existing db this build would not migrate, without writing to it or its directory.
/// A db a newer build migrated is not refused; the outcome says so.
///
/// A db with no `-wal` file is read as immutable, because a read-only open of a WAL-mode db
/// creates the `-wal` and `-shm` files.
pub(super) fn refuse_other_version(db: &Path) -> Result<Outcome, StoreError> {
    if !db.try_exists()? {
        return Ok(Outcome::Current);
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
    )
    .map_err(error)?;
    moor_migrate::check(&conn, &schema()?, legacy).map_err(|e| refused(db, e))
}

/// Bring the db to the latest migration; one process migrates while others wait. A db a newer
/// build migrated is left as found.
pub(super) fn migrate(conn: &mut Connection, db: &Path) -> Result<Outcome, StoreError> {
    moor_migrate::migrate(conn, &schema()?, legacy).map_err(|e| refused(db, e))
}

/// Whether the db is at the latest migration already.
pub(super) fn is_latest(conn: &Connection, db: &Path) -> Result<bool, StoreError> {
    moor_migrate::is_latest(conn, &schema()?).map_err(|e| refused(db, e))
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

#[cfg(test)]
mod tests;
