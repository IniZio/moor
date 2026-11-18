//! `V90__user_inbox` stores an item per event, once, of three kinds, with no read mark until a
//! person sets one.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A Box and its user from before the step.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO boxes (name, "user", created_at) VALUES ('a', 1, 1);
"#;

/// An item for event 5.
const ITEM: &str =
    "INSERT INTO user_inbox (\"user\", \"box\", event, kind, title, text, created_at) \
                    VALUES (1, 1, 5, 'needs_input', 't', 'q', 9)";

/// The same event again.
const SAME_EVENT: &str =
    "INSERT INTO user_inbox (\"user\", \"box\", event, kind, title, text, created_at) \
                          VALUES (1, 1, 5, 'run_failed', 't', 'q', 9)";

/// A kind the inbox does not hold.
const BAD_KIND: &str =
    "INSERT INTO user_inbox (\"user\", \"box\", event, kind, title, text, created_at) \
                        VALUES (1, 1, 6, 'idled', 't', 'q', 9)";

/// The items as `event|read`.
const ROWS: &str =
    "SELECT CAST(event AS TEXT) || '|' || COALESCE(CAST(read_at AS TEXT), 'unread') \
                    FROM user_inbox ORDER BY id";

#[test]
fn sqlite_holds_one_item_per_event_of_three_kinds() {
    let (before, step) = super::before_and_step("V90__user_inbox", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    conn.execute_batch(ITEM).unwrap();
    let rows = |conn: &Connection| -> Vec<String> {
        let mut stmt = conn.prepare(ROWS).unwrap();
        let got = stmt.query_map([], |r| r.get(0)).unwrap();
        got.collect::<Result<_, _>>().unwrap()
    };
    assert_eq!(rows(&conn), ["5|unread"]);
    assert!(conn.execute_batch(SAME_EVENT).is_err());
    assert!(conn.execute_batch(BAD_KIND).is_err());
    conn.execute_batch("UPDATE user_inbox SET read_at = 12")
        .unwrap();
    assert_eq!(rows(&conn), ["5|12"]);
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_holds_one_item_per_event_of_three_kinds() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V90__user_inbox", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_user_inbox_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<(Vec<String>, bool, bool, Vec<String>), postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        client.batch_execute(ITEM)?;
        let rows = |c: &mut Client| -> Result<Vec<String>, postgres::Error> {
            Ok(c.query(ROWS, &[])?.iter().map(|r| r.get(0)).collect())
        };
        let first = rows(&mut client)?;
        let twice = client.batch_execute(SAME_EVENT).is_err();
        let bad = client.batch_execute(BAD_KIND).is_err();
        client.batch_execute("UPDATE user_inbox SET read_at = 12")?;
        Ok((first, twice, bad, rows(&mut client)?))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (first, twice, bad, read) = got.unwrap();
    assert_eq!(first, ["5|unread"]);
    assert!(twice && bad);
    assert_eq!(read, ["5|12"]);
}
