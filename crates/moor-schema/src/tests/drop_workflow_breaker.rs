//! `V95__drop_workflow_breaker` drops `workflows.disabled_reason` and `workflows.change_seq`,
//! enables a workflow the breaker disabled, and leaves one its owner disabled off.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Three workflows: enabled, disabled by its owner, and disabled by the breaker.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO workflows ("user", name, cron, timezone, repos, prompt, enabled_at, disabled_reason,
    timeout_minutes, created_at, updated_at)
    VALUES (1, 'on', '0 0 1 1 *', 'UTC', '["a/b"]', 'p', 5, NULL, 60, 1, 1),
           (1, 'owner', '0 0 1 1 *', 'UTC', '["a/b"]', 'p', NULL, NULL, 60, 1, 2),
           (1, 'breaker', '0 0 1 1 *', 'UTC', '["a/b"]', 'p', NULL, 'the last 3 runs failed', 60, 1, 7);
"#;

/// Each workflow's name and `enabled_at`, `-` for null.
const STATES: &str = "SELECT name || '|' || COALESCE(CAST(enabled_at AS TEXT), '-') \
    FROM workflows ORDER BY id";

fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let got = stmt.query_map([], |r| r.get(0)).unwrap();
    got.collect::<Result<_, _>>().unwrap()
}

#[test]
fn sqlite_enables_the_breakers_workflow_and_drops_both_columns() {
    let (before, step) = super::before_and_step("V95__drop_workflow_breaker", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    assert_eq!(rows(&conn, STATES), ["on|5", "owner|-", "breaker|7"]);
    let left = rows(
        &conn,
        "SELECT name FROM pragma_table_info('workflows') \
         WHERE name IN ('disabled_reason', 'change_seq')",
    );
    assert!(left.is_empty(), "{left:?}");
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_enables_the_breakers_workflow_and_drops_both_columns() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V95__drop_workflow_breaker", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_drop_workflow_breaker_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<(Vec<String>, i64), postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let states = client
            .query(STATES, &[])?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let columns: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM information_schema.columns \
                 WHERE table_schema = current_schema() AND table_name = 'workflows' \
                 AND column_name IN ('disabled_reason', 'change_seq')",
                &[],
            )?
            .get(0);
        Ok((states, columns))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (states, columns) = got.unwrap();
    assert_eq!(states, ["on|5", "owner|-", "breaker|7"]);
    assert_eq!(columns, 0);
}
