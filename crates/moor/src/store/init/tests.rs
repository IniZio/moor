use std::path::Path;

use rusqlite::Connection;

use super::init_schema;
use crate::store::StoreError;

fn version(conn: &Connection) -> String {
    conn.query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
        r.get(0)
    })
    .unwrap()
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
    let conn = fresh_store();
    assert_eq!(launch_session_column(&conn), Some(("INTEGER".into(), 0)));
    let fk: String = conn
        .query_row(
            "SELECT \"table\" FROM pragma_foreign_key_list('launches') WHERE \"from\" = 'session'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fk, "sessions");
    assert_eq!(version(&conn), "10");
}

#[test]
fn an_older_or_newer_version_is_refused_and_left_as_it_is() {
    for found in ["5", "8", "99"] {
        let mut conn = fresh_store();
        conn.execute("UPDATE meta SET v = ?1 WHERE k = 'schema_version'", [found])
            .unwrap();
        let err = init_schema(&mut conn, Path::new("moor.db")).unwrap_err();
        assert!(matches!(err, StoreError::Schema { .. }), "{err:?}");
        assert_eq!(version(&conn), found);
    }
}

#[test]
fn the_proxy_table_keeps_no_nonce_or_host_port() {
    let conn = fresh_store();
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info('proxy')")
        .unwrap();
    let columns: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(columns.contains(&"box_port".to_owned()), "{columns:?}");
    for gone in ["nonce", "host_port"] {
        assert!(!columns.contains(&gone.to_owned()), "{columns:?}");
    }
}

#[test]
fn a_box_may_have_several_live_sessions() {
    let conn = fresh_store();
    conn.execute_batch(
        "INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES ('b', 'r', 'ready', 1, 1);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (1, 1, 'running', 1, NULL);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (1, 2, 'running', 2, NULL);",
    )
    .unwrap();
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn fk_target(conn: &Connection, table: &str, from: &str) -> String {
    conn.query_row(
        &format!(
            "SELECT \"table\" FROM pragma_foreign_key_list('{table}') WHERE \"from\" = '{from}'"
        ),
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn box_notnull(conn: &Connection) -> i64 {
    count(
        conn,
        "SELECT \"notnull\" FROM pragma_table_info('sessions') WHERE name = 'box'",
    )
}

fn assert_boxless_sessions(conn: &Connection) {
    assert!(conn
        .execute(
            "INSERT INTO sessions (ordinal, state, started_at) VALUES (1, 'running', 1)",
            []
        )
        .is_err());
    for _ in 0..2 {
        conn.execute(
            "INSERT INTO sessions (workdir, ordinal, state, started_at) VALUES ('/w', 1, 'running', 1)",
            [],
        )
        .unwrap();
    }
}

#[test]
fn fresh_store_allows_sessions_without_a_box() {
    let conn = fresh_store();
    assert_eq!(box_notnull(&conn), 0);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM pragma_table_info('sessions') WHERE name = 'workdir'"
        ),
        1
    );
    assert_boxless_sessions(&conn);
}

#[test]
fn fresh_store_links_sessions_to_a_parent_with_an_index() {
    let conn = fresh_store();
    assert_eq!(fk_target(&conn, "sessions", "parent_session"), "sessions");
    let indexed = count(
        &conn,
        "SELECT count(*) FROM pragma_index_list('sessions') l, pragma_index_info(l.name) i \
         WHERE i.name = 'parent_session'",
    );
    assert_eq!(indexed, 1);
}

fn fresh_store() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    init_schema(&mut conn, Path::new("moor.db")).unwrap();
    conn
}

#[test]
fn fresh_store_has_no_front_message_column_or_slack_tables() {
    let conn = fresh_store();
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('bindings')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(columns.iter().any(|c| c == "owner"), "{columns:?}");
    assert!(!columns.iter().any(|c| c == "status_ts"), "{columns:?}");
    let slack = count(
        &conn,
        "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'slack%'",
    );
    assert_eq!(slack, 0);
}
