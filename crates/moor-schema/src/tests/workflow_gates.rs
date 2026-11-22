//! `V98__workflow_gates` keeps every workflow with no gates, and lets a workflow carry them.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Two workflows as the store held them before the step.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO workflows ("user", name, cron, timezone, repos, prompt, timeout_minutes, created_at, updated_at)
    VALUES (1, 'a', '0 9 * * *', 'UTC', '["o/n"]', 'p', 60, 1, 1),
           (1, 'b', NULL, NULL, '["o/n"]', 'p', 60, 1, 2);
"#;

/// A third workflow that carries gates.
const AFTER: &str = r#"
INSERT INTO workflows ("user", name, repos, prompt, timeout_minutes, created_at, updated_at, gates)
    VALUES (1, 'c', '["o/n"]', 'p', 60, 1, 3, '[{"name":"proof","evidence":["pr-*"]}]');
"#;

/// Each workflow's name and gates, `-` for none.
const GATES: &str = "SELECT name || '|' || COALESCE(gates, '-') FROM workflows ORDER BY id";

const EXPECTED: [&str; 3] = ["a|-", "b|-", r#"c|[{"name":"proof","evidence":["pr-*"]}]"#];

fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let got = stmt.query_map([], |r| r.get(0)).unwrap();
    got.collect::<Result<_, _>>().unwrap()
}

#[test]
fn sqlite_gives_every_workflow_no_gates_and_accepts_gates() {
    let (before, step) = super::before_and_step("V98__workflow_gates", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    assert_eq!(rows(&conn, GATES), ["a|-", "b|-"]);
    conn.execute_batch(AFTER).unwrap();
    assert_eq!(rows(&conn, GATES), EXPECTED);
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_gives_every_workflow_no_gates_and_accepts_gates() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V98__workflow_gates", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_workflow_gates_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<(Vec<String>, Vec<String>), postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let rows = |c: &mut Client| -> Result<Vec<String>, postgres::Error> {
            Ok(c.query(GATES, &[])?.iter().map(|r| r.get(0)).collect())
        };
        let first = rows(&mut client)?;
        client.batch_execute(AFTER)?;
        Ok((first, rows(&mut client)?))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (first, all) = got.unwrap();
    assert_eq!(first, ["a|-", "b|-"]);
    assert_eq!(all, EXPECTED);
}
