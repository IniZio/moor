//! Schema creation and stored-value parsing helpers.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::{artifacts, lease, slack, StoreError, SCHEMA_SQL, SCHEMA_VERSION};

/// Add the launch-to-session link, backfilling each launch to its Box's
/// first Session.
fn migrate_v2(tx: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    tx.execute_batch(
        "ALTER TABLE launches ADD COLUMN session INTEGER REFERENCES sessions(id);
         UPDATE launches SET session = (SELECT s.id FROM sessions s
           WHERE s.box = launches.box AND s.ordinal = 1);",
    )?;
    Ok(())
}

/// Rebuild `sessions` with a nullable Box, a parent link and a workdir.
const REBUILD_SESSIONS: &str = "
    CREATE TABLE new_sessions (id INTEGER PRIMARY KEY, box INTEGER REFERENCES boxes(id), ordinal INTEGER NOT NULL,
      agent TEXT, model TEXT, agent_session_id TEXT, workspace_id TEXT, pane TEXT, state TEXT NOT NULL,
      started_at INTEGER NOT NULL, ended_at INTEGER, parent_session INTEGER REFERENCES sessions(id), workdir TEXT,
      CHECK (box IS NOT NULL OR workdir IS NOT NULL), UNIQUE (box, ordinal));
    INSERT INTO new_sessions (id, box, ordinal, agent, model, agent_session_id, workspace_id, pane, state,
      started_at, ended_at)
      SELECT id, box, ordinal, agent, model, agent_session_id, workspace_id, pane, state, started_at, ended_at
      FROM sessions;
    DROP TABLE sessions;
    ALTER TABLE new_sessions RENAME TO sessions;
    CREATE INDEX sessions_parent ON sessions(parent_session);";

/// Drop the proxy column for the Box's secret. Every lease and host port stays: a supervisor that
/// an older moor started on host TCP keeps serving its Box until `ensure` or startup retires it,
/// which happens only once crabbox passes its version check.
const DROP_PROXY_SECRET: &str = "ALTER TABLE proxy DROP COLUMN nonce;";

/// Upgrade a v2, v3 or v4 file in place. Foreign keys are off while the
/// tables change and checked before commit.
fn migrate(conn: &mut Connection, found: &str) -> Result<(), StoreError> {
    let fk: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys = OFF")?;
    let done = rebuild(conn, found);
    let restore = conn.execute_batch(if fk {
        "PRAGMA foreign_keys = ON"
    } else {
        "PRAGMA foreign_keys = OFF"
    });
    done?;
    restore?;
    Ok(())
}

/// Run the upgrade steps in one transaction.
fn rebuild(conn: &mut Connection, found: &str) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now: Option<String> = tx
        .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
            r.get(0)
        })
        .optional()?;
    if now.as_deref() != Some(found) {
        return Ok(());
    }
    if found == "2" {
        migrate_v2(&tx)?;
    }
    if found != "4" {
        tx.execute_batch(REBUILD_SESSIONS)?;
    }
    tx.execute_batch(DROP_PROXY_SECRET)?;
    let broken = tx.prepare("PRAGMA foreign_key_check")?.exists([])?;
    if broken {
        return Err(StoreError::Corrupt(
            "foreign key check failed while upgrading the store".to_owned(),
        ));
    }
    tx.execute(
        "UPDATE meta SET v = ?1 WHERE k = 'schema_version'",
        [SCHEMA_VERSION],
    )?;
    tx.execute_batch(slack::SLACK_SQL)?;
    tx.execute_batch(lease::LEASE_SQL)?;
    tx.execute_batch(artifacts::ARTIFACTS_SQL)?;
    tx.commit()?;
    Ok(())
}

/// Read the stored schema version, `None` when `meta` or the row is absent.
fn stored_version(conn: &Connection) -> Result<Option<String>, StoreError> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |r| r.get(0),
    )?;
    if !has_meta {
        return Ok(None);
    }
    let sql = "SELECT v FROM meta WHERE k = 'schema_version'";
    Ok(conn.query_row(sql, [], |r| r.get(0)).optional()?)
}

/// Create the schema when `meta` is absent, else check the version.
pub(super) fn init_schema(conn: &mut Connection) -> Result<(), StoreError> {
    let found = stored_version(conn)?;
    if let Some(old @ ("2" | "3" | "4")) = found.as_deref() {
        return migrate(conn, old);
    }
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
