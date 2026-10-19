use rusqlite::Connection;

use super::init_schema;
use crate::store::{StoreError, SCHEMA_SQL};

fn version(conn: &Connection) -> String {
    conn.query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
        r.get(0)
    })
    .unwrap()
}

fn previous_store() -> Connection {
    let sql = SCHEMA_SQL
        .replace(
            ",\n  session INTEGER REFERENCES sessions(id));\nCREATE TABLE box_refs",
            ");\nCREATE TABLE box_refs",
        )
        .replace("('schema_version', '3')", "('schema_version', '2')");
    assert!(sql.contains("'2'"));
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&sql).unwrap();
    conn
}

fn launch_session_column(conn: &Connection) -> Option<(String, i64)> {
    conn.query_row(
        "SELECT type, \"notnull\" FROM pragma_table_info('launches') WHERE name = 'session'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

#[test]
fn fresh_store_links_launches_to_sessions() {
    let mut conn = Connection::open_in_memory().unwrap();
    init_schema(&mut conn).unwrap();
    assert_eq!(launch_session_column(&conn), Some(("INTEGER".into(), 0)));
    let fk: String = conn
        .query_row(
            "SELECT \"table\" FROM pragma_foreign_key_list('launches') WHERE \"from\" = 'session'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fk, "sessions");
    assert_eq!(version(&conn), "3");
}

#[test]
fn previous_store_migrates_and_keeps_rows() {
    let mut conn = previous_store();
    assert_eq!(launch_session_column(&conn), None);
    conn.execute_batch(
        "INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES ('b', 'r', 'ready', 1, 1);
         INSERT INTO sessions (box, ordinal, state, started_at) VALUES (1, 1, 'running', 1);
         INSERT INTO launches (box, session_ref, started_at) VALUES (1, 'ref', 2);",
    )
    .unwrap();
    init_schema(&mut conn).unwrap();
    assert_eq!(version(&conn), "3");
    let (box_name, sref, session): (String, String, Option<i64>) = conn
        .query_row(
            "SELECT b.name, l.session_ref, l.session FROM launches l JOIN boxes b ON b.id = l.box",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (box_name.as_str(), sref.as_str(), session),
        ("b", "ref", Some(1))
    );
    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sessions, 1);
}

#[test]
fn other_versions_are_rejected() {
    let mut conn = previous_store();
    conn.execute("UPDATE meta SET v = '99' WHERE k = 'schema_version'", [])
        .unwrap();
    let err = init_schema(&mut conn).unwrap_err();
    assert!(matches!(err, StoreError::Schema { .. }), "{err:?}");
}

#[test]
fn a_box_may_have_several_live_sessions() {
    let mut conn = Connection::open_in_memory().unwrap();
    init_schema(&mut conn).unwrap();
    conn.execute_batch(
        "INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES ('b', 'r', 'ready', 1, 1);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (1, 1, 'running', 1, NULL);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (1, 2, 'running', 2, NULL);",
    )
    .unwrap();
}
