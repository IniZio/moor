//! `V103__task_boxes` leaves every Box tied to no task, and lets a Box name only an existing Box as
//! its task.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A user, a place and a work Box, as the store held them before the step.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO boxes (name, kind, "user", created_at) VALUES ('p', 'place', 1, 1);
INSERT INTO boxes (name, "user", created_at) VALUES ('a', 1, 2);
"#;

/// A Box started by place 1.
const TIED: &str = "INSERT INTO boxes (name, \"user\", task, created_at) VALUES ('w', 1, 1, 3)";

/// A Box started by a place that does not exist.
const NO_PLACE: &str = "INSERT INTO boxes (name, \"user\", task, created_at) VALUES ('x', 1, 9, 3)";

/// Each Box as `name|task`, `-` for none.
const TASKS: &str =
    "SELECT name || '|' || COALESCE(CAST(task AS TEXT), '-') FROM boxes ORDER BY id";

/// The names of the Boxes place 1 started, by the index's column.
const STARTED: &str = "SELECT name FROM boxes WHERE task = 1 ORDER BY id";

/// The existing Boxes' tasks, the Boxes place 1 started, and whether the write that names no
/// place was refused.
type Seen = (Vec<String>, Vec<String>, bool);

#[test]
fn sqlite_leaves_an_old_box_with_no_task_and_refuses_a_task_that_is_no_box() {
    let (before, step) = super::before_and_step("V103__task_boxes", Backend::Sqlite);
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
    assert_eq!(rows(TASKS), ["p|-", "a|-"]);
    conn.execute_batch(TIED).unwrap();
    assert!(conn.execute_batch(NO_PLACE).is_err());
    assert_eq!(rows(STARTED), ["w"]);
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_leaves_an_old_box_with_no_task_and_refuses_a_task_that_is_no_box() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V103__task_boxes", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_task_boxes_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Seen, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let rows = |c: &mut Client, sql: &str| -> Result<Vec<String>, postgres::Error> {
            Ok(c.query(sql, &[])?.iter().map(|r| r.get(0)).collect())
        };
        let old = rows(&mut client, TASKS)?;
        client.batch_execute(TIED)?;
        let refused = client.batch_execute(NO_PLACE).is_err();
        Ok((old, rows(&mut client, STARTED)?, refused))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (old, started, refused) = got.unwrap();
    assert_eq!(old, ["p|-", "a|-"]);
    assert_eq!(started, ["w"]);
    assert!(refused);
}
