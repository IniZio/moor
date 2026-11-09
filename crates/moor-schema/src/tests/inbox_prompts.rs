//! `V73__inbox_prompts` makes the queued prompt rows match the held prompts again, on both
//! backends: rows copied by `V72__box_inbox` whose prompt was given since go, prompts held since
//! get a row, and every other row stays.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Box 1 still holds its copied prompt, Box 2's was given, Box 3 held one after the copy, and
/// Box 4 has a queued steer.
const BEFORE: &str = "
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO boxes (name, created_at) VALUES ('b', 1);
INSERT INTO boxes (name, created_at) VALUES ('c', 1);
INSERT INTO boxes (name, created_at) VALUES ('d', 1);
INSERT INTO box_state (box, state, open_name, held_prompt, updated_at)
    VALUES (1, 'ready', 'a', 'fix the build', 5);
INSERT INTO box_state (box, state, open_name, updated_at) VALUES (2, 'ready', 'b', 6);
INSERT INTO box_state (box, state, open_name, held_prompt, updated_at)
    VALUES (3, 'ready', 'c', 'ship it', 7);
INSERT INTO box_state (box, state, open_name, updated_at) VALUES (4, 'ready', 'd', 8);
INSERT INTO box_inbox (box, kind, text, sender, created_at)
    VALUES (1, 'prompt', 'fix the build', 'start', 5);
INSERT INTO box_inbox (box, kind, text, sender, created_at)
    VALUES (2, 'prompt', 'given already', 'start', 6);
INSERT INTO box_inbox (box, kind, text, sender, created_at) VALUES (4, 'steer', 'go', 'steer', 8);
";

/// Each inbox row's Box, kind, text and send count.
const INBOX: &str = "SELECT CAST(box AS TEXT) || '|' || kind || '|' || text || '|' \
    || CAST(attempts AS TEXT) FROM box_inbox ORDER BY box, id";

fn expected() -> Vec<&'static str> {
    vec![
        "1|prompt|fix the build|0",
        "3|prompt|ship it|0",
        "4|steer|go|0",
    ]
}

#[test]
fn sqlite_queues_each_prompt_still_held_once() {
    let (before, step) = super::before_and_step("V73__inbox_prompts", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let mut stmt = conn.prepare(INBOX).unwrap();
    let got: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(got, expected());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_queues_each_prompt_still_held_once() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V73__inbox_prompts", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_inbox_prompts_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Vec<String>, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        Ok(client.query(INBOX, &[])?.iter().map(|r| r.get(0)).collect())
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), expected());
}
