//! `V72__box_inbox` turns each held prompt into its Box's first inbox row, queued, and leaves
//! `held_prompt` as it was, on both backends.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Box 1 holds a prompt for its asleep agent, Box 2 holds none.
const BEFORE: &str = "
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO boxes (name, created_at) VALUES ('b', 1);
INSERT INTO box_state (box, state, open_name, held_prompt, updated_at)
    VALUES (1, 'ready', 'a', 'fix the build', 5);
INSERT INTO box_state (box, state, open_name, updated_at) VALUES (2, 'ready', 'b', 6);
";

/// Each inbox row, `-` for null.
const INBOX: &str =
    "SELECT CAST(box AS TEXT) || '|' || kind || '|' || text || '|' || sender || '|' \
    || CAST(created_at AS TEXT) || '|' || CAST(attempts AS TEXT) || '|' \
    || COALESCE(CAST(launch AS TEXT), '-') || '|' || COALESCE(CAST(sent_at AS TEXT), '-') \
    FROM box_inbox ORDER BY id";

/// Each Box's held prompt, `-` for none.
const HELD: &str =
    "SELECT CAST(box AS TEXT) || '|' || COALESCE(held_prompt, '-') FROM box_state ORDER BY box";

fn expected() -> [Vec<&'static str>; 2] {
    [
        vec!["1|prompt|fix the build|start|5|0|-|-"],
        vec!["1|fix the build", "2|-"],
    ]
}

#[test]
fn sqlite_queues_each_held_prompt_and_keeps_it_held() {
    let (before, step) = super::before_and_step("V72__box_inbox", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let rows = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let got = stmt.query_map([], |r| r.get(0)).unwrap();
        got.collect::<Result<_, _>>().unwrap()
    };
    assert_eq!([rows(INBOX), rows(HELD)], expected());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_queues_each_held_prompt_and_keeps_it_held() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V72__box_inbox", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_box_inbox_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<[Vec<String>; 2], postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let mut rows = |sql: &str| -> Result<Vec<String>, postgres::Error> {
            Ok(client.query(sql, &[])?.iter().map(|r| r.get(0)).collect())
        };
        Ok([rows(INBOX)?, rows(HELD)?])
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), expected());
}
