use std::collections::BTreeMap;

use rusqlite::{types::Value, Connection};

use super::init_schema;
use crate::store::{lease, slack, StoreError};

const V3_SQL: &str = include_str!("v3.sql");

fn version(conn: &Connection) -> String {
    conn.query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
        r.get(0)
    })
    .unwrap()
}

fn v3_store() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(V3_SQL).unwrap();
    conn.execute_batch(slack::SLACK_SQL).unwrap();
    conn.execute_batch(lease::LEASE_SQL).unwrap();
    conn
}

fn previous_store() -> Connection {
    let sql = V3_SQL
        .replace(
            ",\n  session INTEGER REFERENCES sessions(id));\nCREATE TABLE box_refs",
            ");\nCREATE TABLE box_refs",
        )
        .replace("('schema_version', '3')", "('schema_version', '2')");
    assert!(sql.contains("'2'"));
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&sql).unwrap();
    conn.execute_batch(slack::SLACK_SQL).unwrap();
    conn.execute_batch(lease::LEASE_SQL).unwrap();
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
    assert_eq!(version(&conn), "4");
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
    assert_eq!(version(&conn), "4");
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
    let mut conn = Connection::open_in_memory().unwrap();
    init_schema(&mut conn).unwrap();
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
    let mut conn = Connection::open_in_memory().unwrap();
    init_schema(&mut conn).unwrap();
    assert_eq!(fk_target(&conn, "sessions", "parent_session"), "sessions");
    let indexed = count(
        &conn,
        "SELECT count(*) FROM pragma_index_list('sessions') l, pragma_index_info(l.name) i \
         WHERE i.name = 'parent_session'",
    );
    assert_eq!(indexed, 1);
}

const V3_ROWS: &str = "
    INSERT INTO boxes (id, name, repo, state, created_at, updated_at) VALUES (1, 'a', 'r', 'ready', 1, 1);
    INSERT INTO boxes (id, name, repo, state, created_at, updated_at) VALUES (2, 'b', 'r', 'ready', 1, 1);
    INSERT INTO sessions (id, box, ordinal, agent, model, agent_session_id, workspace_id, pane, state, started_at, ended_at)
      VALUES (1, 1, 1, 'claude', 'opus', 'as1', 'w1', 'p1', 'running', 10, NULL);
    INSERT INTO sessions (id, box, ordinal, agent, model, agent_session_id, workspace_id, pane, state, started_at, ended_at)
      VALUES (2, 1, 2, 'codex', 'gpt', 'as2', 'w2', 'p2', 'ended', 11, 12);
    INSERT INTO sessions (id, box, ordinal, agent, model, agent_session_id, workspace_id, pane, state, started_at, ended_at)
      VALUES (5, 2, 1, NULL, NULL, NULL, NULL, NULL, 'running', 13, NULL);
    INSERT INTO launches (id, box, session_ref, started_at, session) VALUES (1, 1, 'ref1', 20, 2);
    INSERT INTO launches (id, box, session_ref, started_at, session) VALUES (2, 2, 'ref2', 21, 5);
    INSERT INTO events (id, box, session, kind, payload, ts) VALUES (1, 1, 1, 'k', '{}', 30);
    INSERT INTO events (id, box, session, kind, payload, ts) VALUES (2, 2, 5, 'k', '{}', 31);
    INSERT INTO reports (id, box, session, kind, version, path, created_at) VALUES (1, 1, 2, 'k', 1, '/p', 40);
    INSERT INTO hook_cache (session, record, at) VALUES (5, 'rec', 50);
    INSERT INTO bindings (front, conv_key, session, owner, created_at) VALUES ('slack', 'c1', 1, 'u', 60);
    INSERT INTO slack_prompts (event_id, channel, thread_ts, message_ts, session, question, options, created_at)
      VALUES (1, 'C', 't', 'm', 5, 'q?', '[]', 61);";

fn migrated_v3() -> (Connection, Vec<String>) {
    let mut conn = v3_store();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    conn.execute_batch(V3_ROWS).unwrap();
    let before = sessions_dump(&conn);
    init_schema(&mut conn).unwrap();
    (conn, before)
}

#[test]
fn v3_migration_keeps_sessions_and_referencing_rows() {
    let (conn, before) = migrated_v3();
    assert_eq!(version(&conn), "4");
    assert_eq!(before.len(), 3);
    assert_eq!(sessions_dump(&conn), before);
    for (table, rows) in [
        ("launches", 2),
        ("events", 2),
        ("reports", 1),
        ("hook_cache", 1),
        ("bindings", 1),
        ("slack_prompts", 1),
    ] {
        let sql = format!("SELECT count(*) FROM {table} t JOIN sessions s ON s.id = t.session");
        assert_eq!(count(&conn, &sql), rows, "{table}");
    }
    assert_eq!(count(&conn, "SELECT session FROM launches WHERE id = 1"), 2);
    assert_eq!(count(&conn, "SELECT session FROM slack_prompts"), 5);
}

#[test]
fn v3_migration_leaves_foreign_keys_intact_and_enforced() {
    let (conn, _) = migrated_v3();
    assert_eq!(
        count(&conn, "SELECT count(*) FROM pragma_foreign_key_check"),
        0
    );
    assert_eq!(count(&conn, "PRAGMA foreign_keys"), 1);
    for table in ["launches", "events", "reports", "hook_cache"] {
        assert_eq!(fk_target(&conn, table, "session"), "sessions", "{table}");
    }
    let orphan = conn.execute(
        "INSERT INTO launches (box, session_ref, started_at, session) VALUES (1, 'x', 1, 999)",
        [],
    );
    assert!(orphan.is_err());
}

#[test]
fn v3_migration_allows_sessions_without_a_box() {
    let (conn, _) = migrated_v3();
    assert_eq!(box_notnull(&conn), 0);
    assert_boxless_sessions(&conn);
}

fn sessions_dump(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT id || '|' || box || '|' || ordinal || '|' || ifnull(agent, '-') || '|' || ifnull(model, '-') \
             || '|' || ifnull(agent_session_id, '-') || '|' || ifnull(workspace_id, '-') || '|' || ifnull(pane, '-') \
             || '|' || state || '|' || started_at || '|' || ifnull(ended_at, '-') \
             FROM sessions WHERE box IS NOT NULL ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

type Shape = BTreeMap<String, String>;

fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let n = stmt.column_count();
    let mut out: Vec<String> = stmt
        .query_map([], |r| {
            let cells: Vec<Value> = (0..n).map(|i| r.get(i)).collect::<Result<_, _>>()?;
            Ok(format!("{cells:?}"))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    out.sort();
    out
}

fn table_shape(conn: &Connection, t: &str) -> String {
    let cols = rows(
        conn,
        &format!(
            "SELECT cid, name, type, \"notnull\", dflt_value, pk, hidden FROM pragma_table_xinfo('{t}') ORDER BY cid"
        ),
    );
    let fks = rows(
        conn,
        &format!(
            "SELECT \"table\", \"from\", \"to\", on_update, on_delete, \"match\" FROM pragma_foreign_key_list('{t}')"
        ),
    );
    let idx = rows(
        conn,
        &format!(
            "SELECT l.name, l.\"unique\", l.origin, l.partial, \
             (SELECT group_concat(i.name, ',') FROM pragma_index_info(l.name) i) \
             FROM pragma_index_list('{t}') l"
        ),
    );
    format!("{cols:?}\n{fks:?}\n{idx:?}")
}

fn shape(conn: &Connection) -> Shape {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    names
        .into_iter()
        .map(|t| {
            let body = table_shape(conn, &t);
            (t, body)
        })
        .collect()
}

fn fresh_store() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    init_schema(&mut conn).unwrap();
    conn
}

fn migrated_shape(mut conn: Connection) -> Shape {
    init_schema(&mut conn).unwrap();
    shape(&conn)
}

#[test]
fn migrated_v3_store_matches_a_fresh_schema() {
    assert_eq!(migrated_shape(v3_store()), shape(&fresh_store()));
}

#[test]
fn migrated_v2_store_matches_a_fresh_schema() {
    assert_eq!(migrated_shape(previous_store()), shape(&fresh_store()));
}

#[test]
fn failed_foreign_key_check_keeps_v3_and_enforcement() {
    let mut conn = v3_store();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO hook_cache (session, record, at) VALUES (999, 'rec', 1);
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
    let err = init_schema(&mut conn).unwrap_err();
    assert!(matches!(err, StoreError::Corrupt(_)), "{err:?}");
    assert_eq!(version(&conn), "3");
    assert_eq!(count(&conn, "PRAGMA foreign_keys"), 1);
}

fn table_counts(conn: &Connection) -> Vec<i64> {
    ["boxes", "sessions", "launches", "events", "hook_cache"]
        .iter()
        .map(|t| count(conn, &format!("SELECT count(*) FROM {t}")))
        .collect()
}

#[test]
fn rebuild_is_a_no_op_when_another_process_already_upgraded() {
    let mut conn = fresh_store();
    conn.execute_batch(
        "INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES ('b', 'r', 'ready', 1, 1);
         INSERT INTO sessions (box, ordinal, state, started_at) VALUES (1, 1, 'running', 1);",
    )
    .unwrap();
    let (before_shape, before_counts) = (shape(&conn), table_counts(&conn));
    super::rebuild(&mut conn, "3").unwrap();
    assert_eq!(shape(&conn), before_shape);
    assert_eq!(table_counts(&conn), before_counts);
    assert_eq!(version(&conn), "4");
}

const V2_ROWS: &str = "
    INSERT INTO boxes (id, name, repo, state, created_at, updated_at) VALUES (1, 'b', 'r', 'ready', 1, 1);
    INSERT INTO sessions (id, box, ordinal, state, started_at) VALUES (7, 1, 1, 'running', 1);
    INSERT INTO launches (id, box, session_ref, started_at) VALUES (1, 1, 'ref', 2);
    INSERT INTO events (id, box, session, kind, payload, ts) VALUES (1, 1, 7, 'k', '{}', 3);
    INSERT INTO reports (id, box, session, kind, version, path, created_at) VALUES (1, 1, 7, 'k', 1, '/p', 4);
    INSERT INTO hook_cache (session, record, at) VALUES (7, 'rec', 5);
    INSERT INTO bindings (front, conv_key, session, owner, created_at) VALUES ('slack', 'c', 7, 'u', 6);
    INSERT INTO slack_prompts (event_id, channel, thread_ts, message_ts, session, question, options, created_at)
      VALUES (1, 'C', 't', 'm', 7, 'q?', '[]', 7);";

#[test]
fn v2_migration_keeps_every_referencing_table() {
    let mut conn = previous_store();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    conn.execute_batch(V2_ROWS).unwrap();
    init_schema(&mut conn).unwrap();
    assert_eq!(version(&conn), "4");
    for table in [
        "launches",
        "events",
        "reports",
        "hook_cache",
        "bindings",
        "slack_prompts",
    ] {
        let sql = format!("SELECT count(*) FROM {table} t JOIN sessions s ON s.id = t.session");
        assert_eq!(count(&conn, &sql), 1, "{table}");
        let ids = format!("SELECT session FROM {table}");
        assert_eq!(count(&conn, &ids), 7, "{table}");
    }
    assert_eq!(
        count(&conn, "SELECT count(*) FROM pragma_foreign_key_check"),
        0
    );
}
