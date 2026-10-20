//! Schema creation and stored-value parsing helpers.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::{artifacts, lease, slack, StoreError, SCHEMA_SQL, SCHEMA_VERSION};

/// Create the schema when `meta` is absent, else refuse any version but [`SCHEMA_VERSION`].
pub(super) fn init_schema(conn: &mut Connection, db: &Path) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_meta: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |r| r.get(0),
    )?;
    if has_meta {
        let sql = "SELECT v FROM meta WHERE k = 'schema_version'";
        let found: Option<String> = tx.query_row(sql, [], |r| r.get(0)).optional()?;
        if found.as_deref() != Some(SCHEMA_VERSION) {
            return Err(StoreError::Schema {
                db: db.to_path_buf(),
                found: found.unwrap_or_else(|| "none".to_owned()),
                want: SCHEMA_VERSION,
            });
        }
    } else {
        tx.execute_batch(SCHEMA_SQL)?;
    }
    tx.execute_batch(slack::SLACK_SQL)?;
    tx.execute_batch(lease::LEASE_SQL)?;
    tx.execute_batch(artifacts::ARTIFACTS_SQL)?;
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
