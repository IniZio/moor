//! `V80__shared_machines` lets a machine have no owner and keeps every machine, its id, and the
//! link, Box and memory rows that cascade from it, on both backends. The index over shared names
//! keeps one shared machine per name, and an owned machine still goes in as an older binary
//! writes it.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Two users' machines, the second with a link, a Box record and a memory row.
const BEFORE: &str = r#"
INSERT INTO users (created_at) VALUES (1);
INSERT INTO users (created_at) VALUES (2);
INSERT INTO user_machines_v2 ("user", name, pubkey, capacity, enrolled_at)
    VALUES (1, 'gone', 'k0', 1, 1);
INSERT INTO user_machines_v2 ("user", name, pubkey, labels, capacity, enrolled_at)
    VALUES (1, 'mini', 'k1', '["a"]', 2, 10);
INSERT INTO user_machines_v2 ("user", name, pubkey, labels, capacity, enrolled_at, pool_provider)
    VALUES (2, 'mini', 'k2', '[]', 4, 20, 'crabbox');
DELETE FROM user_machines_v2 WHERE name = 'gone';
INSERT INTO machine_links (machine_id, online, since, runner_addr) VALUES (3, 1, 5, '/run/r.sock');
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO machine_boxes (box_id, machine_id, slot, reach, facts, prepared, evicted, updated_at)
    VALUES (1, 3, 0, '{}', '{}', 1, 0, 1);
INSERT INTO machine_box_memory (machine_id, box_id, host_memory_mib) VALUES (3, 1, 512);
"#;

/// Statements an older binary writes after the step, and the operator's shared machines.
const AFTER: &str = r#"
INSERT INTO user_machines_v2 ("user", name, pubkey, capacity, enrolled_at)
    VALUES (1, 'other', 'k3', 1, 30);
INSERT INTO user_machines_v2 ("user", name, pubkey, capacity, enrolled_at, pool_provider)
    VALUES (NULL, 'local', 'k4', 1, 40, 'boxes');
INSERT INTO user_machines_v2 ("user", name, pubkey, capacity, enrolled_at)
    VALUES (NULL, 'other', 'k5', 1, 50);
"#;

/// A second shared machine of the same name, which the index refuses.
const DUPLICATE_SHARED: &str = r#"
INSERT INTO user_machines_v2 ("user", name, pubkey, capacity, enrolled_at)
    VALUES (NULL, 'local', 'k6', 1, 60);
"#;

/// Every machine as `id|user|name|pubkey|labels|capacity|enrolled_at|pool`, `-` for null.
const MACHINES: &str =
    "SELECT CAST(machine_id AS TEXT) || '|' || COALESCE(CAST(\"user\" AS TEXT), '-') \
    || '|' || name || '|' || pubkey || '|' || labels || '|' || CAST(capacity AS TEXT) || '|' \
    || CAST(enrolled_at AS TEXT) || '|' || COALESCE(pool_provider, '-') \
    FROM user_machines_v2 ORDER BY machine_id";

/// The rows that cascade from machine 3.
const CHILDREN: [&str; 3] = [
    "SELECT CAST(machine_id AS TEXT) || '|' || CAST(online AS TEXT) || '|' || runner_addr \
     FROM machine_links",
    "SELECT CAST(box_id AS TEXT) || '|' || CAST(machine_id AS TEXT) FROM machine_boxes",
    "SELECT CAST(machine_id AS TEXT) || '|' || CAST(host_memory_mib AS TEXT) \
     FROM machine_box_memory",
];

/// What the Postgres run reads: the machines after the step, the children, the machines after
/// the later writes, and whether the duplicate shared machine was refused.
type Got = (Vec<String>, [Vec<String>; 3], Vec<String>, bool);

fn kept() -> Vec<&'static str> {
    vec!["2|1|mini|k1|[\"a\"]|2|10|-", "3|2|mini|k2|[]|4|20|crabbox"]
}

fn after() -> Vec<&'static str> {
    vec![
        "2|1|mini|k1|[\"a\"]|2|10|-",
        "3|2|mini|k2|[]|4|20|crabbox",
        "4|1|other|k3|[]|1|30|-",
        "5|-|local|k4|[]|1|40|boxes",
        "6|-|other|k5|[]|1|50|-",
    ]
}

fn children() -> [Vec<&'static str>; 3] {
    [vec!["3|1|/run/r.sock"], vec!["1|3"], vec!["3|512"]]
}

#[test]
fn sqlite_keeps_machines_and_their_children_and_takes_shared_machines() {
    let (before, step) = super::before_and_step("V80__shared_machines", Backend::Sqlite);
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
    assert_eq!(rows(MACHINES), kept());
    assert_eq!(CHILDREN.map(rows), children());
    conn.execute_batch(AFTER).unwrap();
    assert_eq!(rows(MACHINES), after());
    assert!(conn.execute_batch(DUPLICATE_SHARED).is_err());
    // The children still cascade from the rebuilt table.
    conn.execute_batch("DELETE FROM user_machines_v2 WHERE machine_id = 3;")
        .unwrap();
    assert_eq!(
        CHILDREN.map(rows),
        [vec![], vec![], vec![]] as [Vec<&str>; 3]
    );
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_keeps_machines_and_their_children_and_takes_shared_machines() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V80__shared_machines", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_shared_machines_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Got, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let mut rows = |sql: &str| -> Result<Vec<String>, postgres::Error> {
            Ok(client.query(sql, &[])?.iter().map(|r| r.get(0)).collect())
        };
        let first = rows(MACHINES)?;
        let [a, b, c] = CHILDREN;
        let children = [rows(a)?, rows(b)?, rows(c)?];
        client.batch_execute(AFTER)?;
        let all = {
            let got = client.query(MACHINES, &[])?;
            got.iter().map(|r| r.get(0)).collect()
        };
        let refused = client.batch_execute(DUPLICATE_SHARED).is_err();
        Ok((first, children, all, refused))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (first, kids, all, refused) = got.unwrap();
    assert_eq!(first, kept());
    assert_eq!(kids, children());
    assert_eq!(all, after());
    assert!(refused);
}
