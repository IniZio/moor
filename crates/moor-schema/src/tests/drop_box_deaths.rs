//! `V89__drop_box_deaths` drops `box_state.recent_deaths` and keeps every Box's row.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A Box with deaths recorded.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO boxes (name, "user", created_at) VALUES ('a', 1, 1);
INSERT INTO box_state ("box", state, updated_at, recent_deaths) VALUES (1, 'ready', 1, '[1,2]');
"#;

/// How many columns `box_state` has called `recent_deaths`, as SQLite counts them.
const COLUMN_SQLITE: &str =
    "SELECT COUNT(*) FROM pragma_table_info('box_state') WHERE name = 'recent_deaths'";

#[test]
fn sqlite_drops_the_column_and_keeps_the_row() {
    let (before, step) = super::before_and_step("V89__drop_box_deaths", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    let count =
        |conn: &Connection, sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(count(&conn, COLUMN_SQLITE), 1);
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    assert_eq!(count(&conn, COLUMN_SQLITE), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM box_state"), 1);
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_drops_the_column_and_keeps_the_row() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V89__drop_box_deaths", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_drop_box_deaths_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<(i64, i64), postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let columns: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM information_schema.columns \
                 WHERE table_schema = current_schema() AND table_name = 'box_state' \
                 AND column_name = 'recent_deaths'",
                &[],
            )?
            .get(0);
        let rows: i64 = client
            .query_one("SELECT COUNT(*) FROM box_state", &[])?
            .get(0);
        Ok((columns, rows))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), (0, 1));
}
