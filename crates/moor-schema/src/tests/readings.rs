//! `V108__readings` leaves every workflow as it was with no reading, and keeps a reading once.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Two workflows as the store held them before the step, one with gates.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO workflows ("user", name, repos, prompt, timeout_minutes, created_at, updated_at, gates)
    VALUES (1, 'a', '["o/n"]', 'p', 60, 1, 1, '[{"name":"proof"}]'),
           (1, 'b', '["o/n"]', 'p', 60, 1, 2, NULL);
"#;

/// A reading of the first workflow, and the workflow pointing at it.
const AFTER: &str = r"
INSERT INTO readings (id, of_workflow, owner, source, context, state, since, read)
    VALUES ('r1', 1, 1, 'p', '{}', 'read', 5, '{}');
UPDATE workflows SET reading = 'r1' WHERE id = 1;
";

/// Each workflow's name, gates and reading, `-` for none.
const HELD: &str =
    "SELECT name || '|' || COALESCE(gates, '-') || '|' || COALESCE(reading, '-') FROM workflows ORDER BY id";

const BEFORE_ROWS: [&str; 2] = [r#"a|[{"name":"proof"}]|-"#, "b|-|-"];
const AFTER_ROWS: [&str; 2] = [r#"a|[{"name":"proof"}]|r1"#, "b|-|-"];

/// The workflows before the reading, with it, whether a second reading of its id was refused, and
/// the readings left once the workflow is gone.
type Seen = (Vec<String>, Vec<String>, bool, Vec<String>);

fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let got = stmt.query_map([], |r| r.get(0)).unwrap();
    got.collect::<Result<_, _>>().unwrap()
}

#[test]
fn sqlite_keeps_every_workflow_and_a_reading_goes_with_its_workflow() {
    let (before, step) = super::before_and_step("V108__readings", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    assert_eq!(rows(&conn, HELD), BEFORE_ROWS);
    assert_eq!(rows(&conn, "SELECT id FROM readings"), Vec::<String>::new());
    conn.execute_batch(AFTER).unwrap();
    assert_eq!(rows(&conn, HELD), AFTER_ROWS);
    assert!(conn
        .execute_batch("INSERT INTO readings (id, owner, source, context, state, since) VALUES ('r1', 1, 'p', '{}', 'read', 6)")
        .is_err());
    conn.execute_batch("DELETE FROM workflows WHERE id = 1")
        .unwrap();
    assert_eq!(rows(&conn, "SELECT id FROM readings"), Vec::<String>::new());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_keeps_every_workflow_and_a_reading_goes_with_its_workflow() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V108__readings", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_readings_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Seen, postgres::Error> {
        let rows = |c: &mut Client, sql: &str| -> Result<Vec<String>, postgres::Error> {
            Ok(c.query(sql, &[])?.iter().map(|r| r.get(0)).collect())
        };
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let first = rows(&mut client, HELD)?;
        client.batch_execute(AFTER)?;
        let held = rows(&mut client, HELD)?;
        let twice = client
            .batch_execute("INSERT INTO readings (id, owner, source, context, state, since) VALUES ('r1', 1, 'p', '{}', 'read', 6)")
            .is_err();
        let mut fresh = Client::connect(&url, NoTls)?;
        fresh.batch_execute(&format!("SET search_path TO {name}"))?;
        fresh.batch_execute("DELETE FROM workflows WHERE id = 1")?;
        Ok((
            first,
            held,
            twice,
            rows(&mut fresh, "SELECT id FROM readings")?,
        ))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (first, held, twice, left) = got.unwrap();
    assert_eq!(first, BEFORE_ROWS);
    assert_eq!(held, AFTER_ROWS);
    assert!(twice);
    assert!(left.is_empty());
}
