//! `V102__box_places` leaves every Box a work Box, takes a place as the other kind, and keeps at
//! most one claim row for each place.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A user and two Boxes, as the store held them before the step.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO boxes (name, "user", created_at) VALUES ('a', 1, 1), ('b', 1, 2);
"#;

/// A third Box, born a place.
const PLACE: &str = "INSERT INTO boxes (name, kind, created_at) VALUES ('p', 'place', 3)";

/// A Box of a kind the store does not hold.
const BAD_KIND: &str = "INSERT INTO boxes (name, kind, created_at) VALUES ('q', 'idle', 3)";

/// A claim of Box 3.
const CLAIM: &str = "INSERT INTO place_claims (\"box\", \"user\", repos, prompt, claimed_at) \
    VALUES (3, 1, '[\"o/n\"]', 'where is it', 9)";

/// The same place claimed again.
const CLAIM_AGAIN: &str =
    "INSERT INTO place_claims (\"box\", \"user\", repos, prompt, claimed_at) \
    VALUES (3, 1, '[\"o/m\"]', 'again', 10)";

/// A claim of a Box that does not exist.
const CLAIM_NOBODY: &str =
    "INSERT INTO place_claims (\"box\", \"user\", repos, prompt, claimed_at) \
    VALUES (9, 1, '[\"o/n\"]', 'q', 9)";

/// Each Box as `name|kind`.
const KINDS: &str = "SELECT name || '|' || kind FROM boxes ORDER BY id";

/// Each claim as `box|prompt|served|steered`, `-` for unset.
const CLAIMS: &str = "SELECT CAST(\"box\" AS TEXT) || '|' || prompt || '|' \
    || COALESCE(CAST(served_at AS TEXT), '-') || '|' || COALESCE(CAST(steered_at AS TEXT), '-') \
    FROM place_claims ORDER BY \"box\"";

/// The kinds before the step's new rows, the claims after, and whether each bad write was refused.
type Seen = (Vec<String>, Vec<String>, [bool; 3]);

#[test]
fn sqlite_reads_an_old_box_as_work_and_keeps_one_claim_per_place() {
    let (before, step) = super::before_and_step("V102__box_places", Backend::Sqlite);
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
    assert_eq!(rows(KINDS), ["a|work", "b|work"]);
    assert!(rows(CLAIMS).is_empty());
    conn.execute_batch(PLACE).unwrap();
    assert!(conn.execute_batch(BAD_KIND).is_err());
    conn.execute_batch(CLAIM).unwrap();
    assert_eq!(rows(CLAIMS), ["3|where is it|-|-"]);
    assert!(conn.execute_batch(CLAIM_AGAIN).is_err());
    assert!(conn.execute_batch(CLAIM_NOBODY).is_err());
    assert_eq!(rows(KINDS), ["a|work", "b|work", "p|place"]);
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_reads_an_old_box_as_work_and_keeps_one_claim_per_place() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V102__box_places", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_box_places_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Seen, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let rows = |c: &mut Client, sql: &str| -> Result<Vec<String>, postgres::Error> {
            Ok(c.query(sql, &[])?.iter().map(|r| r.get(0)).collect())
        };
        let old = rows(&mut client, KINDS)?;
        client.batch_execute(PLACE)?;
        let bad = client.batch_execute(BAD_KIND).is_err();
        client.batch_execute(CLAIM)?;
        let twice = client.batch_execute(CLAIM_AGAIN).is_err();
        let nobody = client.batch_execute(CLAIM_NOBODY).is_err();
        Ok((old, rows(&mut client, CLAIMS)?, [bad, twice, nobody]))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (old, claims, refused) = got.unwrap();
    assert_eq!(old, ["a|work", "b|work"]);
    assert_eq!(claims, ["3|where is it|-|-"]);
    assert_eq!(refused, [true; 3]);
}
