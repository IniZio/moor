//! `V87__workflow_run_trigger` reads every old run as fired by cron, takes `manual` for a run
//! fired by hand, and refuses any other trigger.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A workflow with two runs from before the step.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO workflows ("user", name, cron, timezone, repos, prompt, timeout_minutes, created_at, updated_at)
    VALUES (1, 'w', '0 0 1 1 *', 'UTC', '["a/b"]', 'p', 60, 1, 1);
INSERT INTO workflow_runs (workflow, repo, due_at, outcome, recorded_at)
    VALUES (1, 'a/b', 1, 'starting', 1), (1, 'a/b', 2, 'skipped_overlap', 2);
"#;

/// A run fired by hand, and one an older binary writes with no trigger.
const AFTER: &str = r"
INSERT INTO workflow_runs (workflow, repo, due_at, outcome, recorded_at, trigger)
    VALUES (1, 'a/b', 3, 'starting', 3, 'manual');
INSERT INTO workflow_runs (workflow, repo, due_at, outcome, recorded_at)
    VALUES (1, 'a/b', 4, 'starting', 4);
";

/// A trigger the check does not list.
const BAD_TRIGGER: &str =
    "INSERT INTO workflow_runs (workflow, repo, due_at, outcome, recorded_at, trigger) \
     VALUES (1, 'a/b', 9, 'starting', 9, 'webhook')";

/// Every run as `due_at|trigger`.
const TRIGGERS: &str =
    "SELECT CAST(due_at AS TEXT) || '|' || trigger FROM workflow_runs ORDER BY due_at";

#[test]
fn sqlite_reads_old_runs_as_cron_and_takes_manual() {
    let (before, step) = super::before_and_step("V87__workflow_run_trigger", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let rows = || -> Vec<String> {
        let mut stmt = conn.prepare(TRIGGERS).unwrap();
        let got = stmt.query_map([], |r| r.get(0)).unwrap();
        got.collect::<Result<_, _>>().unwrap()
    };
    assert_eq!(rows(), ["1|cron", "2|cron"]);
    conn.execute_batch(AFTER).unwrap();
    assert_eq!(rows(), ["1|cron", "2|cron", "3|manual", "4|cron"]);
    assert!(conn.execute_batch(BAD_TRIGGER).is_err());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_reads_old_runs_as_cron_and_takes_manual() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V87__workflow_run_trigger", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_workflow_run_trigger_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<(Vec<String>, Vec<String>, bool), postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let rows = |c: &mut Client| -> Result<Vec<String>, postgres::Error> {
            Ok(c.query(TRIGGERS, &[])?.iter().map(|r| r.get(0)).collect())
        };
        let first = rows(&mut client)?;
        client.batch_execute(AFTER)?;
        let all = rows(&mut client)?;
        Ok((first, all, client.batch_execute(BAD_TRIGGER).is_err()))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (first, all, refused) = got.unwrap();
    assert_eq!(first, ["1|cron", "2|cron"]);
    assert_eq!(all, ["1|cron", "2|cron", "3|manual", "4|cron"]);
    assert!(refused);
}
