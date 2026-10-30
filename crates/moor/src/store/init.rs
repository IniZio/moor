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

/// moor.db's migration files, oldest first. Never edit an applied one; add a new file.
const FILES: &[(&str, &str)] = &[
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
    (
        "V9__machine_sleep.sql",
        include_str!("../../migrations/V9__machine_sleep.sql"),
    ),
    (
        "V10__mcp_machine.sql",
        include_str!("../../migrations/V10__mcp_machine.sql"),
    ),
    (
        "V11__mcp_oauth.sql",
        include_str!("../../migrations/V11__mcp_oauth.sql"),
    ),
    (
        "V12__mcp_oauth_sign_out.sql",
        include_str!("../../migrations/V12__mcp_oauth_sign_out.sql"),
    ),
    (
        "V13__drop_mcp_machine.sql",
        include_str!("../../migrations/V13__drop_mcp_machine.sql"),
    ),
    (
        "V14__machine_claim_state.sql",
        include_str!("../../migrations/V14__machine_claim_state.sql"),
    ),
    (
        "V15__box_prompt.sql",
        include_str!("../../migrations/V15__box_prompt.sql"),
    ),
    (
        "V16__user_email.sql",
        include_str!("../../migrations/V16__user_email.sql"),
    ),
    (
        "V17__user_machines_v2.sql",
        include_str!("../../migrations/V17__user_machines_v2.sql"),
    ),
    (
        "V18__machine_links.sql",
        include_str!("../../migrations/V18__machine_links.sql"),
    ),
    (
        "V19__machine_claim_enrolled.sql",
        include_str!("../../migrations/V19__machine_claim_enrolled.sql"),
    ),
    (
        "V20__user_profile.sql",
        include_str!("../../migrations/V20__user_profile.sql"),
    ),
    (
        "V21__box_profile.sql",
        include_str!("../../migrations/V21__box_profile.sql"),
    ),
    (
        "V22__machine_kind.sql",
        include_str!("../../migrations/V22__machine_kind.sql"),
    ),
    (
        "V23__machine_pool.sql",
        include_str!("../../migrations/V23__machine_pool.sql"),
    ),
    (
        "V24__machine_link_version.sql",
        include_str!("../../migrations/V24__machine_link_version.sql"),
    ),
];

/// moor.db's migrations, oldest first.
pub(super) fn schema() -> Result<Vec<Migration>, StoreError> {
    moor_migrate::migrations(FILES)
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
