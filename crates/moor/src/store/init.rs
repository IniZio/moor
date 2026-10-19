//! Schema creation and stored-value parsing helpers.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::{lease, slack, StoreError, SCHEMA_SQL, SCHEMA_VERSION};

/// The schema version that `migrate_previous` upgrades in place.
const PREVIOUS_VERSION: &str = "2";

/// Add the launch-to-session link, backfilling each launch to its Box's
/// first Session, and stamp the current version.
fn migrate_previous(tx: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    tx.execute_batch(
        "ALTER TABLE launches ADD COLUMN session INTEGER REFERENCES sessions(id);
         UPDATE launches SET session = (SELECT s.id FROM sessions s
           WHERE s.box = launches.box AND s.ordinal = 1);",
    )?;
    tx.execute(
        "UPDATE meta SET v = ?1 WHERE k = 'schema_version'",
        [SCHEMA_VERSION],
    )?;
    Ok(())
}

/// Create the schema when `meta` is absent, else check the version.
pub(super) fn init_schema(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_meta: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |r| r.get(0),
    )?;
    if has_meta {
        let sql = "SELECT v FROM meta WHERE k = 'schema_version'";
        let found: Option<String> = tx.query_row(sql, [], |r| r.get(0)).optional()?;
        if found.as_deref() == Some(PREVIOUS_VERSION) {
            migrate_previous(&tx)?;
        } else if found.as_deref() != Some(SCHEMA_VERSION) {
            return Err(StoreError::Schema {
                found: found.unwrap_or_else(|| "none".to_owned()),
                want: SCHEMA_VERSION,
            });
        }
    } else {
        tx.execute_batch(SCHEMA_SQL)?;
    }
    tx.execute_batch(slack::SLACK_SQL)?;
    tx.execute_batch(lease::LEASE_SQL)?;
    tx.commit()?;
    Ok(())
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
