//! `V70__hands_ended` only adds a nullable column: hands written before it keep every value, and
//! hands written after it by a binary that knows nothing of `ended_at` still go in, on both
//! backends.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A Box whose first hands were lost, as the store held them before the step.
const BEFORE: &str = "
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO box_hands (box, inc, provider, handle, state, state_at, created_at)
    VALUES (1, 1, 'p', 'h1', 'gone', 5, 2);
";

/// The Box's second hands, written as a binary without the step writes them.
const AFTER: &str = "
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (1, 2, 'p', 'starting', 7, 7);
";

/// Each hands row, old columns then `ended_at`, `-` for null.
const HANDS: &str = "SELECT CAST(box AS TEXT) || '|' || CAST(inc AS TEXT) || '|' || provider \
    || '|' || COALESCE(handle, '-') || '|' || state || '|' || CAST(state_at AS TEXT) || '|' \
    || COALESCE(CAST(ended_at AS TEXT), '-') FROM box_hands ORDER BY box, inc";

fn expected() -> Vec<&'static str> {
    vec!["1|1|p|h1|gone|5|-", "1|2|p|-|starting|7|-"]
}

#[test]
fn sqlite_adds_ended_at_and_keeps_old_writes_working() {
    let (before, step) = super::before_and_step("V70__hands_ended", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    conn.execute_batch(AFTER).unwrap();
    let mut stmt = conn.prepare(HANDS).unwrap();
    let got: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(got, expected());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_adds_ended_at_and_keeps_old_writes_working() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V70__hands_ended", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_hands_ended_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Vec<String>, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        client.batch_execute(AFTER)?;
        Ok(client.query(HANDS, &[])?.iter().map(|r| r.get(0)).collect())
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), expected());
}
