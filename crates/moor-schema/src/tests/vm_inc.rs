//! `V71__vm_inc` gives each VM fact row its Box's current hands, none for a Box without hands,
//! and rows a binary that knows no `inc` writes after it still go in, on both backends.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

use crate::vm_inc::TABLES;

/// Box 1 on its second hands, Box 2 with none yet, each with a row in every VM table.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO user_machines_v2 ("user", name, pubkey, capacity, enrolled_at)
    VALUES (1, 'm', 'k', 2, 1);
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO boxes (name, created_at) VALUES ('b', 1);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (1, 1, 'p', 'gone', 1, 1);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (1, 2, 'p', 'ready', 2, 2);
INSERT INTO proxy (box, box_port, egress_port, updated_at) VALUES (1, 20001, 10001, 1);
INSERT INTO proxy (box, box_port, egress_port, updated_at) VALUES (2, 20002, 10002, 1);
INSERT INTO machine_boxes (box_id, machine_id, slot, reach, facts, prepared, evicted, updated_at)
    VALUES (1, 1, 0, '{}', '{}', 1, 0, 1);
INSERT INTO machine_boxes (box_id, machine_id, slot, reach, facts, prepared, evicted, updated_at)
    VALUES (2, 1, 1, '{}', '{}', 0, 0, 1);
INSERT INTO machine_box_memory (machine_id, box_id, host_memory_mib) VALUES (1, 1, 512);
INSERT INTO machine_box_memory (machine_id, box_id, host_memory_mib) VALUES (1, 2, 256);
INSERT INTO box_machines (box, "user", state, machine, ports, slot) VALUES (1, 1, 'held', 'm', '[]', 1);
INSERT INTO box_machines (box, "user", state, ports) VALUES (2, 1, 'wanted', '[]');
INSERT INTO setup_checkpoint_pins (box_id, provider, layer_key, layer) VALUES (1, 'p', 'k1', 0);
INSERT INTO setup_checkpoint_pins (box_id, provider, layer_key, layer) VALUES (2, 'p', 'k2', 0);
"#;

/// A third Box's rows, written as a binary without the step writes them.
const AFTER: &str = r#"
INSERT INTO boxes (name, created_at) VALUES ('c', 1);
INSERT INTO proxy (box, box_port, egress_port, updated_at) VALUES (3, 20003, 10003, 1);
INSERT INTO machine_boxes (box_id, machine_id, slot, reach, facts, prepared, evicted, updated_at)
    VALUES (3, 1, 2, '{}', '{}', 0, 0, 1);
INSERT INTO machine_box_memory (machine_id, box_id, host_memory_mib) VALUES (1, 3, 128);
INSERT INTO box_machines (box, "user", state, ports) VALUES (3, 1, 'wanted', '[]');
INSERT INTO setup_checkpoint_pins (box_id, provider, layer_key, layer) VALUES (3, 'p', 'k3', 0);
"#;

/// Each table's rows as `<box>|<inc>`, `-` for a null `inc`, by Box.
fn queries() -> Vec<String> {
    TABLES
        .iter()
        .map(|(table, of)| {
            format!(
                "SELECT CAST({of} AS TEXT) || '|' || COALESCE(CAST(inc AS TEXT), '-') \
                 FROM {table} ORDER BY {of}"
            )
        })
        .collect()
}

/// Every table: Box 1 at its current hands, Box 2 with none, Box 3 written without `inc`.
fn expected() -> Vec<Vec<&'static str>> {
    vec![vec!["1|2", "2|-", "3|-"]; TABLES.len()]
}

#[test]
fn sqlite_backfills_each_vm_row_with_its_current_hands() {
    let (before, step) = super::before_and_step("V71__vm_inc", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    conn.execute_batch(AFTER).unwrap();
    let rows = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let got = stmt.query_map([], |r| r.get(0)).unwrap();
        got.collect::<Result<_, _>>().unwrap()
    };
    let got: Vec<Vec<String>> = queries().iter().map(|q| rows(q)).collect();
    assert_eq!(got, expected());
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_backfills_each_vm_row_with_its_current_hands() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V71__vm_inc", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_vm_inc_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Vec<Vec<String>>, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        client.batch_execute(AFTER)?;
        queries()
            .iter()
            .map(|q| Ok(client.query(q, &[])?.iter().map(|r| r.get(0)).collect()))
            .collect()
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), expected());
}
