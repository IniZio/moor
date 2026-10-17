//! Idempotent table creation that also upgrades tables made by older builds.

use rusqlite::Connection;

use super::{lease, slack, StoreError};

/// Create the slack and lease tables, adding lease columns an older file lacks.
///
/// `CREATE TABLE IF NOT EXISTS` leaves an existing table as it was, so a state
/// database that planned lease rows before these columns existed needs `ALTER`.
pub(super) fn tables(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(slack::SLACK_SQL)?;
    conn.execute_batch(lease::LEASE_SQL)?;
    add_column(conn, "box_lease", "attempts", "INTEGER NOT NULL DEFAULT 0")?;
    add_column(conn, "box_lease", "failure", "TEXT")
}

/// Add `column` to `table` when `PRAGMA table_info` does not list it.
fn add_column(conn: &Connection, table: &str, column: &str, ty: &str) -> Result<(), StoreError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let has = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == column);
    drop(stmt);
    if !has {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"))?;
    }
    Ok(())
}
