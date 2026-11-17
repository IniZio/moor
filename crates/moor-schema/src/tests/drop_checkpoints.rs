//! `V79__drop_checkpoints` drops the checkpoint tables with their rows and leaves the Boxes and
//! `machine_links.warm_starts` that a controller one release older still writes, on both backends.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// A Box with a pin on a checkpoint.
const BEFORE: &str = "
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO setup_checkpoints (provider, layer_key, checkpoint, repo, bytes, created_at, last_used_at)
    VALUES ('p', 'k', 'chk_1', 'r', 9, 1, 1);
INSERT INTO setup_checkpoint_pins (box_id, provider, layer_key, layer) VALUES (1, 'p', 'k', 0);
";

/// What an older controller still writes: a machine link row with `warm_starts`.
const AFTER: &str = "
INSERT INTO users (created_at) VALUES (1);
INSERT INTO user_machines_v2 (\"user\", name, pubkey, capacity, enrolled_at)
    VALUES (1, 'm', 'k', 2, 1);
INSERT INTO machine_links (machine_id, online, since, warm_starts)
    VALUES (1, 1, 1, '{\"state\":\"on\"}');
";

#[test]
fn sqlite_drops_the_checkpoint_tables_and_keeps_old_link_writes_working() {
    let (before, step) = super::before_and_step("V79__drop_checkpoints", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'setup_checkpoint%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
    conn.execute_batch(AFTER).unwrap();
    let boxes: i64 = conn
        .query_row("SELECT COUNT(*) FROM boxes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(boxes, 1);
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_drops_the_checkpoint_tables_and_keeps_old_link_writes_working() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V79__drop_checkpoints", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_drop_checkpoints_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<i64, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        client.batch_execute(AFTER)?;
        let left = client.query_one(
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = current_schema() AND table_name LIKE 'setup_checkpoint%'",
            &[],
        )?;
        Ok(left.get(0))
    };
    let left = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(left.unwrap(), 0);
}
