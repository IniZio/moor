//! `V88__box_deaths` adds an empty `recent_deaths` to every Box's row and takes a list in it.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A Box from before the step.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO boxes (name, "user", created_at) VALUES ('a', 1, 1);
INSERT INTO box_state ("box", state, updated_at) VALUES (1, 'ready', 1);
"#;

/// Every Box's deaths, or `none`.
const DEATHS: &str = "SELECT COALESCE(recent_deaths, 'none') FROM box_state ORDER BY \"box\"";

#[test]
fn sqlite_starts_with_no_deaths_and_takes_a_list() {
    let (before, step) = super::before_and_step("V88__box_deaths", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let got = |conn: &Connection| -> Vec<String> {
        let mut stmt = conn.prepare(DEATHS).unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.collect::<Result<_, _>>().unwrap()
    };
    assert_eq!(got(&conn), ["none"]);
    conn.execute_batch("UPDATE box_state SET recent_deaths = '[1,2]' WHERE \"box\" = 1")
        .unwrap();
    assert_eq!(got(&conn), ["[1,2]"]);
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_starts_with_no_deaths_and_takes_a_list() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V88__box_deaths", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_box_deaths_test_{}", std::process::id());
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
            Ok(c.query(DEATHS, &[])?.iter().map(|r| r.get(0)).collect())
        };
        let first = rows(&mut client)?;
        client.batch_execute("UPDATE box_state SET recent_deaths = '[1,2]' WHERE \"box\" = 1")?;
        Ok((first, rows(&mut client)?))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (first, then) = got.unwrap();
    assert_eq!(first, ["none"]);
    assert_eq!(then, ["[1,2]"]);
}
