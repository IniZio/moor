//! `V91__workflow_repo_set` folds each workflow's `also` into its `repos` in order, and refuses a
//! store with a workflow of more than one repo.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Three workflows of one repo: with two `also` repos, with none, and with one.
const ONE_REPO_EACH: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO workflows ("user", name, cron, timezone, repos, also, prompt, timeout_minutes, created_at, updated_at)
    VALUES (1, 'a', '0 0 1 1 *', 'UTC', '["o/app"]', '["o/lib","p/deploy"]', 'p', 60, 1, 1),
           (1, 'b', '0 0 1 1 *', 'UTC', '["o/web"]', '[]', 'p', 60, 1, 1),
           (1, 'c', '0 0 1 1 *', 'UTC', '["o/api"]', '["o/docs"]', 'p', 60, 1, 1);
"#;

/// A workflow that starts one Box per repo.
const TWO_REPOS: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO workflows ("user", name, cron, timezone, repos, prompt, timeout_minutes, created_at, updated_at)
    VALUES (1, 'a', '0 0 1 1 *', 'UTC', '["o/app","o/web"]', 'p', 60, 1, 1);
"#;

/// Every workflow as `name|repos|also`.
const ROWS: &str = "SELECT name || '|' || repos || '|' || also FROM workflows ORDER BY name";

/// What the fold leaves.
const FOLDED: [&str; 3] = [
    r#"a|["o/app","o/lib","p/deploy"]|[]"#,
    r#"b|["o/web"]|[]"#,
    r#"c|["o/api","o/docs"]|[]"#,
];

/// An in-memory store through the step before `V91`, with `rows` inserted, then the step's result.
fn sqlite(rows: &str) -> (Connection, Result<(), rusqlite::Error>) {
    let (before, step) = super::before_and_step("V91__workflow_repo_set", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(rows).unwrap();
    let applied = conn.execute_batch(&format!("BEGIN; {step} COMMIT;"));
    (conn, applied)
}

#[test]
fn sqlite_folds_also_after_the_one_repo_in_order() {
    let (conn, applied) = sqlite(ONE_REPO_EACH);
    applied.unwrap();
    let mut stmt = conn.prepare(ROWS).unwrap();
    let got: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(got, FOLDED);
}

#[test]
fn sqlite_refuses_a_workflow_of_two_repos() {
    let (_, applied) = sqlite(TWO_REPOS);
    let why = applied.unwrap_err().to_string();
    assert!(
        why.contains("a_stored_workflow_has_more_than_one_repo"),
        "{why}"
    );
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_folds_also_and_refuses_two_repos() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V91__workflow_repo_set", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_workflow_repo_set_test_{}", std::process::id());
    let mut apply = |rows: &str| -> Result<Vec<String>, postgres::Error> {
        client.batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))?;
        client.batch_execute(&before)?;
        client.batch_execute(rows)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        Ok(client.query(ROWS, &[])?.iter().map(|r| r.get(0)).collect())
    };
    let folded = apply(ONE_REPO_EACH);
    let refused = apply(TWO_REPOS).map_err(|e| e.to_string());
    client
        .batch_execute(&format!("DROP SCHEMA IF EXISTS {name} CASCADE"))
        .unwrap();
    assert_eq!(folded.unwrap(), FOLDED);
    let why = refused.unwrap_err();
    assert!(
        why.contains("a_stored_workflow_has_more_than_one_repo") || why.contains("check"),
        "{why}"
    );
}
