//! `V93__workflow_run_pull` keeps every run, its id and its history, takes a pull request run,
//! keeps one per workflow, repo, pull request and head, keys only cron runs on due time, takes a
//! cancelled run, and refuses a trigger that does not fit its pull request columns.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A workflow with a cron run and a manual run, and two history lines of the first.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO workflows ("user", name, cron, timezone, repos, prompt, timeout_minutes, created_at, updated_at)
    VALUES (1, 'w', '0 0 1 1 *', 'UTC', '["a/b"]', 'p', 60, 1, 1);
INSERT INTO workflow_runs (workflow, repo, due_at, outcome, recorded_at, trigger)
    VALUES (1, 'a/b', 1, 'starting', 1, 'cron'), (1, 'a/b', 2, 'refused', 2, 'manual');
INSERT INTO workflow_run_history (run, n, at, entry)
    VALUES (1, 1, 1, '{"kind":"queued"}'), (1, 2, 2, '{"kind":"skipped"}');
"#;

/// The 40-hex head the inserts use.
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

/// Every run as `id|trigger|pull_number|head_sha|outcome`, `-` for null.
const RUNS: &str = "SELECT CAST(id AS TEXT) || '|' || trigger || '|' || \
    COALESCE(CAST(pull_number AS TEXT), '-') || '|' || COALESCE(head_sha, '-') || '|' || outcome \
    FROM workflow_runs ORDER BY id";

/// The history of run 1.
const HISTORY: &str = "SELECT CAST(n AS TEXT) || '|' || entry FROM workflow_run_history \
    WHERE run = 1 ORDER BY n";

/// A run of `trigger` due at `due` for `pull` and `sha` (SQL text, `NULL` for none), ending
/// `outcome`.
fn run(due: i64, trigger: &str, (pull, sha): (&str, &str), outcome: &str) -> String {
    format!(
        "INSERT INTO workflow_runs (workflow, repo, due_at, outcome, recorded_at, trigger, \
         pull_number, head_sha) VALUES (1, 'a/b', {due}, '{outcome}', {due}, '{trigger}', {pull}, \
         {sha})"
    )
}

fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let got = stmt.query_map([], |r| r.get(0)).unwrap();
    got.collect::<Result<_, _>>().unwrap()
}

#[test]
fn sqlite_keeps_runs_and_history_and_keys_a_pull_request_run_on_its_head() {
    let (before, step) = super::before_and_step("V93__workflow_run_pull", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    assert_eq!(
        rows(&conn, RUNS),
        ["1|cron|-|-|starting", "2|manual|-|-|refused"]
    );
    assert_eq!(
        rows(&conn, HISTORY),
        [r#"1|{"kind":"queued"}"#, r#"2|{"kind":"skipped"}"#]
    );
    let sha = format!("'{SHA}'");
    let other = format!("'{}'", SHA.replace('0', "f"));
    let ok = |sql: String| conn.execute_batch(&sql).map_err(|e| format!("{sql}: {e}"));
    ok(run(3, "pull_request", ("65", &sha), "starting")).unwrap();
    assert!(
        ok(run(4, "pull_request", ("65", &sha), "starting")).is_err(),
        "the same pull request and head records once"
    );
    ok(run(5, "pull_request", ("65", &other), "starting")).unwrap();
    ok(run(6, "pull_request", ("65", &sha), "skipped_fork")).unwrap();
    ok(run(7, "manual", ("65", &sha), "starting")).unwrap();
    ok(run(8, "manual", ("NULL", "NULL"), "starting")).unwrap();
    ok(run(8, "manual", ("NULL", "NULL"), "refused")).unwrap();
    ok(run(3, "pull_request", ("66", &sha), "starting")).unwrap();
    ok(run(10, "pull_request", ("67", &sha), "cancelled")).unwrap();
    assert!(
        ok(run(1, "cron", ("NULL", "NULL"), "starting")).is_err(),
        "a cron run records once per due time"
    );
    for (trigger, subject, outcome) in [
        ("cron", ("65", sha.as_str()), "starting"),
        ("pull_request", ("NULL", "NULL"), "starting"),
        ("manual", ("65", "NULL"), "starting"),
        ("manual", ("65", sha.as_str()), "skipped_fork"),
        ("webhook", ("NULL", "NULL"), "starting"),
    ] {
        assert!(
            ok(run(9, trigger, subject, outcome)).is_err(),
            "{trigger} {subject:?} {outcome}"
        );
    }
    conn.execute_batch("DELETE FROM workflow_runs WHERE id = 1")
        .unwrap();
    assert!(rows(&conn, HISTORY).is_empty(), "history still cascades");
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_keys_a_pull_request_run_on_its_head_and_refuses_a_misfit() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V93__workflow_run_pull", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_workflow_run_pull_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let sha = format!("'{SHA}'");
    let mut go = || -> Result<(Vec<String>, Vec<bool>), postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let kept = client.query(RUNS, &[])?.iter().map(|r| r.get(0)).collect();
        let tries = [
            run(3, "pull_request", ("65", &sha), "starting"),
            run(4, "pull_request", ("65", &sha), "starting"),
            run(5, "pull_request", ("65", &sha), "skipped_fork"),
            run(6, "manual", ("65", &sha), "starting"),
            run(7, "cron", ("65", &sha), "starting"),
            run(8, "manual", ("65", "NULL"), "starting"),
            run(6, "manual", ("NULL", "NULL"), "starting"),
            run(3, "pull_request", ("66", &sha), "cancelled"),
            run(1, "cron", ("NULL", "NULL"), "starting"),
        ];
        let took = tries
            .iter()
            .map(|sql| client.batch_execute(sql).is_ok())
            .collect();
        Ok((kept, took))
    };
    let got = go();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (kept, took) = got.unwrap();
    assert_eq!(kept, ["1|cron|-|-|starting", "2|manual|-|-|refused"]);
    assert_eq!(
        took,
        [true, false, true, true, false, false, true, true, false]
    );
}
