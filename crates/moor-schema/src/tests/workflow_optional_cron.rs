//! `V96__workflow_optional_cron` lets `workflows.cron` and `workflows.timezone` be null, and
//! keeps every workflow, its id, and its runs and their history.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Two workflows with a run each, and a history line on the first run.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO workflows ("user", name, cron, timezone, repos, prompt, enabled_at, timeout_minutes,
    created_at, updated_at)
    VALUES (1, 'a', '0 9 * * *', 'UTC', '["o/n"]', 'p', 5, 60, 1, 1),
           (1, 'b', '0 8 * * *', 'Asia/Hong_Kong', '["o/n"]', 'p', NULL, 60, 1, 2);
INSERT INTO workflow_runs (workflow, repo, due_at, outcome, recorded_at)
    VALUES (1, 'o/n', 10, 'finished', 10), (2, 'o/n', 20, 'finished', 20);
INSERT INTO workflow_run_history (run, n, at, entry) VALUES (1, 1, 10, 'x');
"#;

/// After the step: a workflow with no cron, and one more run and line to see ids still hold.
const AFTER: &str = r#"
INSERT INTO workflows ("user", name, cron, timezone, repos, prompt, enabled_at, timeout_minutes,
    created_at, updated_at)
    VALUES (1, 'c', NULL, NULL, '["o/n"]', 'p', 5, 60, 1, 3);
"#;

/// Each workflow as `name|cron|timezone|enabled_at|runs|history`, `-` for null.
const STATES: &str = "SELECT w.name || '|' || COALESCE(w.cron, '-') || '|' \
    || COALESCE(w.timezone, '-') || '|' || COALESCE(CAST(w.enabled_at AS TEXT), '-') || '|' \
    || (SELECT COUNT(*) FROM workflow_runs r WHERE r.workflow = w.id) || '|' \
    || (SELECT COUNT(*) FROM workflow_run_history h JOIN workflow_runs r ON r.id = h.run \
        WHERE r.workflow = w.id) FROM workflows w ORDER BY w.id";

const EXPECTED: [&str; 3] = [
    "a|0 9 * * *|UTC|5|1|1",
    "b|0 8 * * *|Asia/Hong_Kong|-|1|0",
    "c|-|-|5|0|0",
];

fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let got = stmt.query_map([], |r| r.get(0)).unwrap();
    got.collect::<Result<_, _>>().unwrap()
}

#[test]
fn sqlite_keeps_every_workflow_run_and_history_and_accepts_no_cron() {
    let (before, step) = super::before_and_step("V96__workflow_optional_cron", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    conn.execute_batch(AFTER).unwrap();
    assert_eq!(rows(&conn, STATES), EXPECTED);
    let fk_errors = rows(&conn, "SELECT \"table\" FROM pragma_foreign_key_check");
    assert!(fk_errors.is_empty(), "{fk_errors:?}");
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_keeps_every_workflow_run_and_history_and_accepts_no_cron() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V96__workflow_optional_cron", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_workflow_optional_cron_test_{}", std::process::id());
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
        Ok(client
            .query(STATES, &[])?
            .iter()
            .map(|r| r.get(0))
            .collect())
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), EXPECTED);
}
