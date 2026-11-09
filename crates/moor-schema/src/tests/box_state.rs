//! A populated store keeps every Box and moves each Box's lifecycle to `box_state` through
//! `V68__box_state`, on both backends.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Boxes as the store held them before the move: a closed Box and a rejected one sharing a
/// ready Box's name and branch, an asleep Box with its VM, and a pending Box that never had one.
const BOXES: &str = "
INSERT INTO boxes (name, branch, state, created_at, updated_at, prompt)
    VALUES ('a', 'feat', 'closed', 1, 2, 'old');
INSERT INTO boxes (name, branch, state, created_at, updated_at, node, prompt, intent_kind,
    intent_pid, intent_at, idle_since)
    VALUES ('a', 'feat', 'ready', 3, 4, 'e3', 'p', 'start', 7, 9, 10);
INSERT INTO boxes (name, branch, state, created_at, updated_at)
    VALUES ('a', 'feat', 'rejected', 5, 6);
INSERT INTO boxes (name, state, state_reason, created_at, updated_at, node, held_prompt,
    intent_run, setup_failed_at, steps_pending_at, launch_failure, resume_first_at)
    VALUES ('b', 'asleep', 'idle', 7, 8, 'e3', 'later', 3, 11, 12, '{}', 13);
INSERT INTO boxes (name, state, created_at, updated_at, node)
    VALUES ('c', 'pending', 9, 10, 'e4');
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (2, 1, 'local', 'ready', 30, 30);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (4, 1, 'local', 'asleep', 40, 40);
";

/// Each Box's spec: id, name, branch and when it was made.
const SPEC: &str = "SELECT CAST(id AS TEXT) || '|' || name || '|' || COALESCE(branch, '-') || '|' \
                    || CAST(created_at AS TEXT) FROM boxes ORDER BY id";

/// Each Box's current VM, by Box id.
const HANDS: &str = "SELECT CAST(box AS TEXT) || '|' || state FROM box_hands ORDER BY box, inc";

/// The `box_state` columns, as each row's values joined, `-` for null.
fn state_query() -> String {
    let cols = [
        "box",
        "state",
        "state_reason",
        "open_name",
        "open_branch",
        "node",
        "prompt",
        "held_prompt",
        "intent_kind",
        "intent_pid",
        "intent_run",
        "intent_at",
        "setup_failed_at",
        "steps_pending_at",
        "idle_since",
        "launch_failure",
        "resume_first_at",
        "updated_at",
    ]
    .map(|c| format!("COALESCE(CAST({c} AS TEXT), '-')"))
    .join(" || '|' || ");
    format!("SELECT {cols} FROM box_state ORDER BY box")
}

/// The rows the move must leave: every Box's spec and VM unchanged, and one state row per Box
/// with its name and branch held only while it is live.
fn expected() -> [Vec<&'static str>; 3] {
    [
        vec![
            "1|a|feat|1",
            "2|a|feat|3",
            "3|a|feat|5",
            "4|b|-|7",
            "5|c|-|9",
        ],
        vec!["2|ready", "4|asleep"],
        vec![
            "1|closed|-|-|-|-|old|-|-|-|-|-|-|-|-|-|-|2",
            "2|ready|-|a|feat|e3|p|-|start|7|-|9|-|-|10|-|-|4",
            "3|rejected|-|-|-|-|-|-|-|-|-|-|-|-|-|-|-|6",
            "4|asleep|idle|b|-|e3|-|later|-|-|3|-|11|12|-|{}|13|8",
            "5|pending|-|c|-|e4|-|-|-|-|-|-|-|-|-|-|-|10",
        ],
    ]
}

#[test]
fn sqlite_moves_each_box_lifecycle_to_box_state() {
    let (before, step) = super::before_and_step("V68__box_state", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BOXES).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let rows = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let got = stmt.query_map([], |r| r.get(0)).unwrap();
        got.collect::<Result<_, _>>().unwrap()
    };
    assert_eq!([rows(SPEC), rows(HANDS), rows(&state_query())], expected());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_moves_each_box_lifecycle_to_box_state() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V68__box_state", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_box_state_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<[Vec<String>; 3], postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BOXES)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let mut rows = |sql: &str| -> Result<Vec<String>, postgres::Error> {
            Ok(client.query(sql, &[])?.iter().map(|r| r.get(0)).collect())
        };
        Ok([rows(SPEC)?, rows(HANDS)?, rows(&state_query())?])
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), expected());
}
