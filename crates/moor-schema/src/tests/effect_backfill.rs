//! `V76__effect_backfill` fills each open Box's effect and wanted close from the intent the
//! binary before it wrote, and drops an intent left on a closed Box, on both backends.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Box 1 acquires and asked twice since its start began; Box 2 sets up; Box 3 runs run 7; Box 4
/// is being force-closed; Box 5 launches; Box 6 closed while launching; Box 7 acquires, asked
/// once.
const BEFORE: &str = "
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO boxes (name, created_at) VALUES ('b', 1);
INSERT INTO boxes (name, created_at) VALUES ('c', 1);
INSERT INTO boxes (name, created_at) VALUES ('d', 1);
INSERT INTO boxes (name, created_at) VALUES ('e', 1);
INSERT INTO boxes (name, created_at) VALUES ('f', 1);
INSERT INTO boxes (name, created_at) VALUES ('g', 1);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_at)
    VALUES (1, 'setting_up', 1, 'provisioning', 9, 100);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_at)
    VALUES (2, 'setting_up', 1, 'setup', 9, 200);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_run, intent_at)
    VALUES (3, 'ready', 1, 'running', 9, 7, 300);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_at)
    VALUES (4, 'setting_up', 1, 'closing', 9, 400);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_at)
    VALUES (5, 'ready', 1, 'launching', 9, 500);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_at)
    VALUES (6, 'closed', 1, 'launching', 9, 600);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_at)
    VALUES (7, 'setting_up', 1, 'provisioning', 9, 700);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'box_requested', '{}', 1);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'setting_up', '{}', 2);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'box_requested', '{}', 3);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'box_requested', '{}', 4);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'setting_up', '{}', 2);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'box_requested', '{}', 3);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'box_requested', '{}', 4);
INSERT INTO events (box, kind, payload, ts) VALUES (7, 'box_requested', '{}', 1);
INSERT INTO events (box, kind, payload, ts) VALUES (7, 'setting_up', '{}', 2);
INSERT INTO events (box, kind, payload, ts) VALUES (7, 'box_requested', '{}', 3);
";

/// The wanted and effect columns by Box, `-` for null.
const STATE: &str = "SELECT CAST(box AS TEXT) || '|' || COALESCE(wanted, '-') || '|' \
    || COALESCE(CAST(wanted_force AS TEXT), '-') || '|' || COALESCE(CAST(wanted_at AS TEXT), '-') \
    || '|' || COALESCE(effect, '-') || '|' || COALESCE(CAST(effect_attempt AS TEXT), '-') || '|' \
    || COALESCE(CAST(effect_run AS TEXT), '-') || '|' || COALESCE(CAST(effect_at AS TEXT), '-') \
    || '|' || COALESCE(intent_kind, '-') FROM box_state ORDER BY box";

fn expected() -> Vec<&'static str> {
    vec![
        "1|-|-|-|acquire|2|-|100|-",
        "2|-|-|-|setup|1|-|200|-",
        "3|-|-|-|run|1|7|300|-",
        "4|close|1|400|-|-|-|-|-",
        "5|-|-|-|launch|1|-|500|-",
        "6|-|-|-|-|-|-|-|-",
        "7|-|-|-|acquire|1|-|700|-",
    ]
}

#[test]
fn sqlite_fills_effect_and_wanted_from_the_intent() {
    let (before, step) = super::before_and_step("V76__effect_backfill", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let mut stmt = conn.prepare(STATE).unwrap();
    let got: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(got, expected());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_fills_effect_and_wanted_from_the_intent() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V76__effect_backfill", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_effect_backfill_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Vec<String>, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        Ok(client.query(STATE, &[])?.iter().map(|r| r.get(0)).collect())
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), expected());
}
