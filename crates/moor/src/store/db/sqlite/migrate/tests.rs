use std::path::Path;

use rusqlite::Connection;

use super::migrate as init_schema;
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
    assert_eq!(version(&conn), "16");
}

#[test]
fn an_older_or_newer_version_is_refused_and_left_as_it_is() {
    for found in ["5", "8", "99"] {
        let mut conn = fresh_store();
        conn.execute("UPDATE meta SET v = ?1 WHERE k = 'schema_version'", [found])
            .unwrap();
        conn.execute_batch("DROP TABLE refinery_schema_history")
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

/// A file the SQL migrations brought to `through`: their schema and a history ending there.
fn sql_migrated_file(dir: &Path, through: i32) -> std::path::PathBuf {
    let path = dir.join("moor.db");
    let sql = moor_migrate::render(
        moor_schema::STEPS.first().unwrap(),
        moor_migrate::Backend::Sqlite,
    );
    let name = format!("V{through}__old");
    let old = moor_migrate::migrations(&[(name.as_str(), sql.as_str())]).unwrap();
    let mut conn = Connection::open(&path).unwrap();
    moor_migrate::migrate(&mut conn, &old, |_| Ok(moor_migrate::Legacy::Empty)).unwrap();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (1, 1); \
         INSERT INTO boxes (name, state, created_at, updated_at) VALUES ('kept', 'pending', 1, 1);",
    )
    .unwrap();
    path
}

fn history(conn: &Connection) -> Vec<i64> {
    let mut stmt = conn
        .prepare("SELECT version FROM refinery_schema_history ORDER BY version")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

#[test]
fn a_file_at_the_last_sql_migration_is_adopted_with_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = sql_migrated_file(dir.path(), 44);
    super::refuse_other_version(&path).unwrap();
    let mut conn = Connection::open(&path).unwrap();
    init_schema(&mut conn, &path).unwrap();
    assert_eq!(
        history(&conn),
        [45, 46],
        "recorded at the baseline, then migrated on"
    );
    let name: String = conn
        .query_row("SELECT name FROM boxes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "kept");
}

#[test]
fn a_file_short_of_the_last_sql_migration_is_refused_and_left_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let path = sql_migrated_file(dir.path(), 43);
    let refused = super::refuse_other_version(&path).unwrap_err();
    assert!(matches!(refused, StoreError::Schema { .. }), "{refused:?}");
    let mut conn = Connection::open(&path).unwrap();
    let err = init_schema(&mut conn, &path).unwrap_err().to_string();
    assert!(
        err.contains("migration 43") && err.contains("use a fresh state dir"),
        "{err}"
    );
    assert_eq!(history(&conn), [43]);
}

mod pinned;

#[test]
fn a_store_a_newer_build_migrated_opens_unchanged_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("moor.db");
    drop(crate::store::tests::open(dir.path()));
    let known = *history(&Connection::open(&path).unwrap()).last().unwrap();
    let newer = known + 1;
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
         VALUES (?1, 'future', '2030-01-01T00:00:00Z', '1')",
        [newer],
    )
    .unwrap();
    drop(conn);
    let (store, _) = crate::store::tests::open(dir.path());
    assert_eq!(
        store.schema(),
        moor_migrate::Outcome::Ahead {
            found: newer,
            known
        }
    );
    let after = Connection::open(&path).unwrap();
    assert_eq!(history(&after).last(), Some(&newer), "history untouched");
}
