//! Schema creation and stored-value parsing helpers.

use std::path::Path;

use moor_core::cache::CacheSalt;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use super::open::sibling;
use super::{artifacts, bindings, lease, users, Store, StoreError, SCHEMA_SQL, SCHEMA_VERSION};

impl Store {
    /// This state dir's cache key salt, made on first use and never changed.
    pub(crate) fn cache_salt(&self) -> Result<CacheSalt, StoreError> {
        let mut fresh = [0u8; 64];
        getrandom::fill(&mut fresh)
            .map_err(|e| StoreError::Corrupt(format!("no randomness for the cache salt: {e}")))?;
        let stored: String = {
            let conn = self.writer()?;
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

/// Refuse an existing db at another schema version without writing to it or its directory.
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
    has_current_meta(&conn, db).map(drop)
}

/// Create the schema when `meta` is absent, else refuse any version but [`SCHEMA_VERSION`].
pub(super) fn init_schema(conn: &mut Connection, db: &Path) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !has_current_meta(&tx, db)? {
        tx.execute_batch(SCHEMA_SQL)?;
    }
    tx.execute_batch(bindings::BINDINGS_SQL)?;
    tx.execute_batch(lease::LEASE_SQL)?;
    tx.execute_batch(artifacts::ARTIFACTS_SQL)?;
    tx.execute_batch(users::front_leases::FRONT_LEASES_SQL)?;
    tx.commit()?;
    Ok(())
}

/// True when `meta` exists; refuses a `meta` whose schema version is not [`SCHEMA_VERSION`].
fn has_current_meta(conn: &Connection, db: &Path) -> Result<bool, StoreError> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |r| r.get(0),
    )?;
    if !has_meta {
        return Ok(false);
    }
    let sql = "SELECT v FROM meta WHERE k = 'schema_version'";
    let found: Option<String> = conn.query_row(sql, [], |r| r.get(0)).optional()?;
    if found.as_deref() != Some(SCHEMA_VERSION) {
        return Err(StoreError::Schema {
            db: db.to_path_buf(),
            found: found.unwrap_or_else(|| "none".to_owned()),
            want: SCHEMA_VERSION,
        });
    }
    Ok(true)
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
