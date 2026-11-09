//! `V69__session_archive` only adds nullable columns: rows written before it keep every value,
//! and rows written after it by a binary that knows none of the new columns still go in, on both
//! backends.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A ready Box with a live Session, as the store held them before the step.
const BEFORE: &str = "
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO box_state (box, state, open_name, idle_since, updated_at)
    VALUES (1, 'ready', 'a', 5, 6);
INSERT INTO sessions (box, ordinal, agent, agent_session_id, state, started_at)
    VALUES (1, 1, 'claude', 'agent-1', 'idle', 3);
";

/// A second Box and Session written as a binary without the step writes them.
const AFTER: &str = "
INSERT INTO boxes (name, created_at) VALUES ('b', 7);
INSERT INTO box_state (box, state, open_name, updated_at) VALUES (2, 'pending', 'b', 8);
INSERT INTO sessions (box, ordinal, agent, state, started_at)
    VALUES (2, 1, 'claude', 'working', 9);
";

/// Each Session's values, old columns then the pointer, `-` for null.
const SESSIONS: &str = "SELECT CAST(box AS TEXT) || '|' || agent || '|' \
    || COALESCE(agent_session_id, '-') || '|' || state || '|' || CAST(started_at AS TEXT) || '|' \
    || COALESCE(archive_key, '-') || '|' || COALESCE(CAST(archive_inc AS TEXT), '-') || '|' \
    || COALESCE(CAST(archive_at AS TEXT), '-') FROM sessions ORDER BY id";

/// Each Box's state values, old columns then `archive_due`, `-` for null.
const STATES: &str = "SELECT CAST(box AS TEXT) || '|' || state || '|' || open_name || '|' \
    || COALESCE(CAST(idle_since AS TEXT), '-') || '|' || CAST(updated_at AS TEXT) || '|' \
    || COALESCE(CAST(archive_due AS TEXT), '-') FROM box_state ORDER BY box";

fn expected() -> [Vec<&'static str>; 2] {
    [
        vec![
            "1|claude|agent-1|idle|3|-|-|-",
            "2|claude|-|working|9|-|-|-",
        ],
        vec!["1|ready|a|5|6|-", "2|pending|b|-|8|-"],
    ]
}

#[test]
fn sqlite_adds_the_archive_columns_and_keeps_old_writes_working() {
    let (before, step) = super::before_and_step("V69__session_archive", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    conn.execute_batch(AFTER).unwrap();
    let rows = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let got = stmt.query_map([], |r| r.get(0)).unwrap();
        got.collect::<Result<_, _>>().unwrap()
    };
    assert_eq!([rows(SESSIONS), rows(STATES)], expected());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_adds_the_archive_columns_and_keeps_old_writes_working() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V69__session_archive", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_session_archive_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<[Vec<String>; 2], postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        client.batch_execute(AFTER)?;
        let mut rows = |sql: &str| -> Result<Vec<String>, postgres::Error> {
            Ok(client.query(sql, &[])?.iter().map(|r| r.get(0)).collect())
        };
        Ok([rows(SESSIONS)?, rows(STATES)?])
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), expected());
}
