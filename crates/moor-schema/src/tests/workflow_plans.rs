//! `V85__workflow_plans` keeps every workflow run and its id, accepts the four new outcomes and
//! refuses any other, adds the run columns, and gives a run a history that goes with it.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A workflow with a run in each of the six old outcomes.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO workflows ("user", name, cron, timezone, repos, prompt, timeout_minutes, created_at, updated_at)
    VALUES (1, 'w', '* * * * *', 'UTC', '["a/b"]', 'p', 60, 1, 1);
INSERT INTO workflow_runs (workflow, repo, due_at, box, outcome, detail, ended_at, recorded_at, seq)
    VALUES (1, 'a/b', 1, NULL, 'starting', NULL, NULL, 1, 1),
           (1, 'a/b', 2, 7, 'started', NULL, NULL, 2, 2),
           (1, 'a/b', 3, 7, 'finished', NULL, 30, 3, 3),
           (1, 'a/b', 4, 7, 'timed_out', NULL, 40, 4, 4),
           (1, 'a/b', 5, NULL, 'skipped_overlap', NULL, NULL, 5, 5),
           (1, 'a/b', 6, NULL, 'refused', 'no', NULL, 6, 6);
"#;

/// A run in each new outcome, and two history lines of run 2.
const AFTER: &str = r#"
INSERT INTO workflow_runs (workflow, repo, due_at, box, outcome, detail, ended_at, recorded_at, seq, step, try_n, waiting_since, ended_step)
    VALUES (1, 'a/b', 7, 7, 'waiting', NULL, NULL, 7, 7, 'ask', 1, 70, NULL),
           (1, 'a/b', 8, 7, 'failed', 'why', 80, 8, 8, NULL, NULL, NULL, 'fix'),
           (1, 'a/b', 9, 7, 'incomplete', NULL, 90, 9, 9, NULL, NULL, NULL, 'fix'),
           (1, 'a/b', 10, 7, 'declined', NULL, 100, 10, 10, NULL, NULL, NULL, 'ask');
INSERT INTO workflow_run_history (run, n, at, entry)
    VALUES (2, 1, 2, '{"kind":"queued"}'), (2, 2, 3, '{"kind":"entered"}');
"#;

/// Every run as `id|due_at|box|outcome|detail|ended_at|seq|step|ended_step`, `-` for null.
const RUNS: &str = "SELECT CAST(id AS TEXT) || '|' || CAST(due_at AS TEXT) || '|' || \
    COALESCE(CAST(box AS TEXT), '-') || '|' || outcome || '|' || COALESCE(detail, '-') || '|' || \
    COALESCE(CAST(ended_at AS TEXT), '-') || '|' || CAST(seq AS TEXT) || '|' || \
    COALESCE(step, '-') || '|' || COALESCE(ended_step, '-') FROM workflow_runs ORDER BY id";

/// A run with an outcome the check does not list.
const BAD_OUTCOME: &str =
    "INSERT INTO workflow_runs (workflow, repo, due_at, outcome, recorded_at) \
     VALUES (1, 'a/b', 99, 'lost', 1)";

/// A second line numbered 1 for run 2.
const DUPLICATE_LINE: &str =
    "INSERT INTO workflow_run_history (run, n, at, entry) VALUES (2, 1, 9, '{}')";

/// The history of run 2.
const HISTORY: &str = "SELECT CAST(n AS TEXT) || '|' || entry FROM workflow_run_history \
    WHERE run = 2 ORDER BY n";

/// The workflow's `revision` and `plan` as `revision|plan`, `-` for a null plan.
const WORKFLOW: &str = "SELECT CAST(revision AS TEXT) || '|' || COALESCE(plan, '-') FROM workflows";

/// Deleting run 2.
const DELETE_RUN: &str = "DELETE FROM workflow_runs WHERE id = 2";

fn kept() -> Vec<&'static str> {
    vec![
        "1|1|-|starting|-|-|1|-|-",
        "2|2|7|started|-|-|2|-|-",
        "3|3|7|finished|-|30|3|-|-",
        "4|4|7|timed_out|-|40|4|-|-",
        "5|5|-|skipped_overlap|-|-|5|-|-",
        "6|6|-|refused|no|-|6|-|-",
    ]
}

fn after() -> Vec<&'static str> {
    let mut all = kept();
    all.extend([
        "7|7|7|waiting|-|-|7|ask|-",
        "8|8|7|failed|why|80|8|-|fix",
        "9|9|7|incomplete|-|90|9|-|fix",
        "10|10|7|declined|-|100|10|-|ask",
    ]);
    all
}

fn history() -> Vec<&'static str> {
    vec!["1|{\"kind\":\"queued\"}", "2|{\"kind\":\"entered\"}"]
}

#[test]
fn sqlite_keeps_runs_and_takes_the_new_outcomes_and_a_history() {
    let (before, step) = super::before_and_step("V85__workflow_plans", Backend::Sqlite);
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
    assert_eq!(rows(RUNS), kept());
    assert_eq!(rows(WORKFLOW), ["0|-"]);
    conn.execute_batch(AFTER).unwrap();
    assert_eq!(rows(RUNS), after());
    assert_eq!(rows(HISTORY), history());
    assert!(conn.execute_batch(BAD_OUTCOME).is_err());
    assert!(conn.execute_batch(DUPLICATE_LINE).is_err());
    conn.execute_batch(DELETE_RUN).unwrap();
    assert!(rows(HISTORY).is_empty());
    // The rebuilt table keeps its unique key and goes with its workflow.
    assert!(conn
        .execute_batch(
            "INSERT INTO workflow_runs (workflow, repo, due_at, outcome, recorded_at) \
             VALUES (1, 'a/b', 3, 'starting', 1)"
        )
        .is_err());
    conn.execute_batch("DELETE FROM workflows").unwrap();
    assert!(rows(RUNS).is_empty());
}

/// What the Postgres run reads: the runs after the step, the runs after the later writes, run
/// 2's history, whether both bad writes were refused, and the history after run 2 was deleted.
type Got = (
    Vec<String>,
    Vec<String>,
    Vec<String>,
    [bool; 2],
    Vec<String>,
);

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_keeps_runs_and_takes_the_new_outcomes_and_a_history() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V85__workflow_plans", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_workflow_plans_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let rows = |c: &mut Client, sql: &str| -> Result<Vec<String>, postgres::Error> {
        Ok(c.query(sql, &[])?.iter().map(|r| r.get(0)).collect())
    };
    let mut run = || -> Result<Got, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let first = rows(&mut client, RUNS)?;
        client.batch_execute(AFTER)?;
        let all = rows(&mut client, RUNS)?;
        let lines = rows(&mut client, HISTORY)?;
        let refused = [
            client.batch_execute(BAD_OUTCOME).is_err(),
            client.batch_execute(DUPLICATE_LINE).is_err(),
        ];
        client.batch_execute(DELETE_RUN)?;
        let gone = rows(&mut client, HISTORY)?;
        Ok((first, all, lines, refused, gone))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (first, all, lines, refused, gone) = got.unwrap();
    assert_eq!(first, kept());
    assert_eq!(all, after());
    assert_eq!(lines, history());
    assert_eq!(refused, [true, true]);
    assert!(gone.is_empty());
}
