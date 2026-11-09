//! A populated store keeps every Box and moves each Box's VM to `box_hands` through
//! `V67__box_hands`, on both backends.

use moor_migrate::{render, Backend};
use postgres::{Client, NoTls};
use rusqlite::Connection;

use crate::STEPS;

/// Boxes as the store held them before the move: a closed Box and a ready one sharing a name,
/// an asleep Box whose suspend was checked, and a pending Box that never had a VM.
const BOXES: &str = "
INSERT INTO boxes (name, state, created_at, updated_at, provider, handle, driver_state,
    driver_updated_at, workdir, cpus, mem_mib)
    VALUES ('a', 'closed', 1, 2, 'local', 'h0', 'deleted', 20, '/w', 1, 512);
INSERT INTO boxes (name, state, created_at, updated_at, provider, handle, driver_state,
    driver_updated_at, workdir, egress, cpus, mem_mib, node_mem_mib)
    VALUES ('a', 'ready', 3, 4, 'local', 'h1', 'ready', 30, '/w', 'enforced', 2, 4096, 8192);
INSERT INTO boxes (name, state, created_at, updated_at, provider, handle, attempted_handle,
    driver_state, driver_updated_at, sleep_reason, suspend_ahead, suspend_dirty,
    suspend_checked_at, suspend_publish)
    VALUES ('b', 'pending', 5, 6, 'cloud', 'h2', 'h9', 'asleep', 40, 'pinned', 3, 1, 39, 1);
INSERT INTO boxes (name, state, created_at, updated_at, node)
    VALUES ('c', 'pending', 7, 8, 'e3');
";

/// Each Box's name, state and node.
const SPEC: &str =
    "SELECT name || '|' || state || '|' || COALESCE(node, '-') FROM boxes ORDER BY id";

/// The `box_hands` columns, as their owning Box's name joined with each value, `-` for null.
fn hands_query() -> String {
    let cols = [
        "inc",
        "provider",
        "handle",
        "attempted_handle",
        "state",
        "state_at",
        "workdir",
        "egress",
        "sleep_reason",
        "cpus",
        "mem_mib",
        "suspend_ahead",
        "suspend_dirty",
        "suspend_checked_at",
        "suspend_publish",
        "created_at",
    ]
    .map(|c| format!("COALESCE(CAST(h.{c} AS TEXT), '-')"))
    .join(" || '|' || ");
    format!(
        "SELECT b.name || '|' || {cols} FROM box_hands h JOIN boxes b ON b.id = h.box \
         ORDER BY h.box, h.inc"
    )
}

/// The rows the move must leave: every Box unchanged, and one VM row per Box that had one.
fn expected() -> (Vec<&'static str>, Vec<&'static str>) {
    (
        vec!["a|closed|-", "a|ready|-", "b|pending|-", "c|pending|e3"],
        vec![
            "a|1|local|h0|-|deleted|20|/w|-|-|1|512|-|-|-|-|20",
            "a|1|local|h1|-|ready|30|/w|enforced|-|2|4096|-|-|-|-|30",
            "b|1|cloud|h2|h9|asleep|40|-|-|pinned|-|-|3|1|39|1|40",
        ],
    )
}

/// The steps before the move, and the move itself, rendered for `backend`.
fn before_and_move(backend: Backend) -> (String, String) {
    let (last, before) = STEPS.split_last().unwrap();
    assert_eq!(last.name, "V67__box_hands", "the move is the newest step");
    let before = before.iter().map(|s| render(s, backend)).collect();
    (before, render(last, backend))
}

#[test]
fn sqlite_moves_each_box_vm_to_box_hands() {
    let (before, step) = before_and_move(Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BOXES).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let rows = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let got = stmt.query_map([], |r| r.get(0)).unwrap();
        got.collect::<Result<_, _>>().unwrap()
    };
    let (spec, hands) = expected();
    assert_eq!(rows(SPEC), spec);
    assert_eq!(rows(&hands_query()), hands);
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_moves_each_box_vm_to_box_hands() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = before_and_move(Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_box_hands_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<(Vec<String>, Vec<String>), postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BOXES)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let mut rows = |sql: &str| -> Result<Vec<String>, postgres::Error> {
            Ok(client.query(sql, &[])?.iter().map(|r| r.get(0)).collect())
        };
        Ok((rows(SPEC)?, rows(&hands_query())?))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (spec, hands) = got.unwrap();
    let (want_spec, want_hands) = expected();
    assert_eq!(spec, want_spec);
    assert_eq!(hands, want_hands);
}
