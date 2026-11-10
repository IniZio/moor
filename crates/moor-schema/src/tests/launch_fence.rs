//! `V74__launch_fence` leaves each Box at most one live launch, its newest, on both backends:
//! an older launch never ended takes the start of the launch after it as its end, a launch begun
//! before the Box's newest hands takes their creation as its end, and every other row stays as
//! it was.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Box 1 has two launches never ended, then one ended; Box 2 has one live launch on its only
/// hands; Box 3 launched twice on hands it lost at 25 and got new hands at 30.
const BEFORE: &str = "
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO boxes (name, created_at) VALUES ('b', 1);
INSERT INTO boxes (name, created_at) VALUES ('c', 1);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (2, 1, 'p', 'ready', 45, 45);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at, ended_at)
    VALUES (3, 1, 'p', 'gone', 25, 2, 25);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (3, 2, 'p', 'starting', 30, 30);
INSERT INTO launches (box, session_ref, started_at) VALUES (1, 's', 10);
INSERT INTO launches (box, session_ref, started_at) VALUES (1, 's', 20);
INSERT INTO launches (box, session_ref, started_at, ended_at, exit) VALUES (1, 's', 30, 35, 0);
INSERT INTO launches (box, session_ref, started_at) VALUES (1, 's', 40);
INSERT INTO launches (box, session_ref, started_at) VALUES (2, 's', 50);
INSERT INTO launches (box, session_ref, started_at) VALUES (3, 's', 10);
INSERT INTO launches (box, session_ref, started_at) VALUES (3, 's', 20);
";

/// Each launch's Box, start and end, `-` for null.
const LAUNCHES: &str = "SELECT CAST(box AS TEXT) || '|' || CAST(started_at AS TEXT) || '|' \
    || COALESCE(CAST(ended_at AS TEXT), '-') FROM launches ORDER BY id";

fn expected() -> Vec<&'static str> {
    vec![
        "1|10|20", "1|20|30", "1|30|35", "1|40|-", "2|50|-", "3|10|20", "3|20|30",
    ]
}

#[test]
fn sqlite_ends_every_launch_but_each_box_newest() {
    let (before, step) = super::before_and_step("V74__launch_fence", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let mut stmt = conn.prepare(LAUNCHES).unwrap();
    let got: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(got, expected());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_ends_every_launch_but_each_box_newest() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V74__launch_fence", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_launch_fence_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Vec<String>, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        Ok(client
            .query(LAUNCHES, &[])?
            .iter()
            .map(|r| r.get(0))
            .collect())
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), expected());
}
