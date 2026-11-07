//! A populated store at the baseline keeps its rows and their order through the migration that
//! gives `bindings` and `box_refs` an `id`.

use moor_migrate::{migrate, steps, Backend, Legacy};
use rusqlite::Connection;

use crate::STEPS;

/// A db at the baseline migration only, holding two Boxes, three bindings of one key and four
/// refs, each inserted out of key order so rowid order differs from key order.
fn baseline_db() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let baseline = steps(STEPS.get(..1).unwrap(), Backend::Sqlite).unwrap();
    migrate(&mut conn, &baseline, |_| Ok(Legacy::Empty)).unwrap();
    conn.execute_batch(
        "INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES ('a', 'r', 'ready', 1, 1);
         INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES ('b', 'r', 'ready', 1, 1);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (2, 1, 'ended', 1, 2);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (1, 1, 'ended', 1, 2);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (1, 2, 'ended', 3, 4);
         INSERT INTO bindings (front, conv_key, session, owner, created_at) VALUES ('cli', 'k', 3, 'U', 5);
         INSERT INTO bindings (front, conv_key, session, owner, created_at) VALUES ('cli', 'k', 1, 'U', 5);
         INSERT INTO bindings (front, conv_key, session, owner, created_at) VALUES ('cli', 'k', 2, 'U', 4);
         INSERT INTO box_refs (box, kind, value) VALUES (1, 'branch', 'zeta');
         INSERT INTO box_refs (box, kind, value) VALUES (1, 'branch', 'alpha');
         INSERT INTO box_refs (box, kind, value) VALUES (1, 'branch', 'mid');
         INSERT INTO box_refs (box, kind, value) VALUES (2, 'pr', '7');",
    )
    .unwrap();
    conn
}

/// The first column of each row `sql` returns, as text.
fn strings(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

/// Both tables' rows in insertion order, ordering by `seq`.
fn ordered(conn: &Connection, seq: &str) -> (Vec<String>, Vec<String>) {
    (
        strings(
            conn,
            &format!(
                "SELECT front || '/' || conv_key || '/' || session || '/' || owner || '/' || \
                 created_at FROM bindings ORDER BY {seq}"
            ),
        ),
        strings(
            conn,
            &format!("SELECT box || '/' || kind || '/' || value FROM box_refs ORDER BY {seq}"),
        ),
    )
}

#[test]
fn the_migration_numbers_existing_rows_in_their_rowid_order() {
    let mut conn = baseline_db();
    let before = ordered(&conn, "rowid");
    assert_eq!(
        before.0.first().unwrap(),
        "cli/k/3/U/5",
        "fixture is out of key order"
    );
    let all = steps(STEPS, Backend::Sqlite).unwrap();
    migrate(&mut conn, &all, |_| Ok(Legacy::Empty)).unwrap();
    assert_eq!(ordered(&conn, "id"), before);
    assert_eq!(
        strings(&conn, "SELECT CAST(id AS TEXT) FROM bindings ORDER BY id"),
        ["1", "2", "3"]
    );
    assert_eq!(
        strings(&conn, "SELECT CAST(id AS TEXT) FROM box_refs ORDER BY id"),
        ["1", "2", "3", "4"]
    );
    conn.execute(
        "INSERT INTO box_refs (box, kind, value) VALUES (1, 'branch', 'last')",
        [],
    )
    .unwrap();
    let last = strings(&conn, "SELECT value FROM box_refs ORDER BY id DESC LIMIT 1");
    assert_eq!(last, ["last"], "a new row sorts after the migrated ones");
}

#[test]
fn the_migration_runs_once_and_a_reopen_changes_nothing() {
    let mut conn = baseline_db();
    let all = steps(STEPS, Backend::Sqlite).unwrap();
    migrate(&mut conn, &all, |_| Ok(Legacy::Empty)).unwrap();
    let rows = |c: &Connection| {
        strings(
            c,
            "SELECT 'g/' || id || '/' || session FROM bindings \
             UNION ALL SELECT 'r/' || id || '/' || value FROM box_refs ORDER BY 1",
        )
    };
    let first = rows(&conn);
    migrate(&mut conn, &all, |_| Ok(Legacy::Empty)).unwrap();
    assert_eq!(rows(&conn), first);
    let history = strings(
        &conn,
        "SELECT CAST(version AS TEXT) FROM refinery_schema_history ORDER BY version",
    );
    assert_eq!(
        history,
        [
            "45", "46", "47", "48", "49", "50", "51", "52", "53", "54", "55", "56", "58", "59",
            "60", "61"
        ]
    );
}
