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
fn a_file_at_migration_22_drops_the_machine_kind_and_keeps_its_machines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let mut conn = Connection::open(&path).unwrap();
    let all = super::schema().unwrap();
    let up_to_22 = all.get(..22).unwrap();
    moor_migrate::migrate(&mut conn, up_to_22, |_| Ok(moor_migrate::Legacy::Empty)).unwrap();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (1, 1);
         INSERT INTO user_machines (user, name, ssh, labels, capacity, kind, vm_provider, \
         created_at, updated_at) VALUES (1, 'hv', 'u@h', '[]', 2, 'vm-host', 'external', 1, 1);",
    )
    .unwrap();
    init_schema(&mut conn, &path).unwrap();
    let columns = |table: &str| -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info(?) ORDER BY cid")
            .unwrap();
        let rows = stmt.query_map([table], |r| r.get(0)).unwrap();
        rows.map(Result::unwrap).collect()
    };
    for table in ["user_machines", "user_machines_v2"] {
        let cols = columns(table);
        assert!(
            cols.contains(&"pool_provider".to_owned()),
            "{table}: {cols:?}"
        );
        assert!(!cols.contains(&"kind".to_owned()), "{table}: {cols:?}");
        assert!(
            !cols.contains(&"vm_provider".to_owned()),
            "{table}: {cols:?}"
        );
    }
    let (capacity, pool): (u16, Option<String>) = conn
        .query_row(
            "SELECT capacity, pool_provider FROM user_machines WHERE name = 'hv'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (capacity, pool),
        (2, None),
        "the row stays, as an ordinary machine"
    );
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

#[test]
fn the_cache_salt_is_made_once_per_state_dir() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let first = crate::store::tests::open(a.path()).0.cache_salt().unwrap();
    let (again, _) = crate::store::tests::open(a.path());
    assert_eq!(again.cache_salt().unwrap(), first, "kept across opens");
    assert_eq!(again.cache_salt().unwrap(), first, "kept across calls");
    let other = crate::store::tests::open(b.path()).0.cache_salt().unwrap();
    assert_ne!(other, first, "a fresh state dir has its own");
}

/// A file exactly as the last build before migrations left it: the baseline SQL and no history.
fn pre_migration_file(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("moor.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(include_str!("../../../migrations/V1__baseline.sql"))
        .unwrap();
    conn.execute_batch(include_str!(
        "../../../migrations/V2__tables_made_on_open.sql"
    ))
    .unwrap();
    conn.execute("INSERT INTO meta (k, v) VALUES ('cache_salt', 'kept')", [])
        .unwrap();
    path
}

#[test]
fn an_adopted_file_gains_the_tables_older_builds_made_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    Connection::open(&path)
        .unwrap()
        .execute_batch(include_str!("../../../migrations/V1__baseline.sql"))
        .unwrap();
    let mut conn = Connection::open(&path).unwrap();
    init_schema(&mut conn, &path).unwrap();
    let names: Vec<String> = tables(&conn).into_iter().map(|(t, _)| t).collect();
    assert!(names.contains(&"front_leases".to_owned()), "{names:?}");
    assert_eq!(version(&conn), "16");
}

/// Tables that migrations after V2 create; a file migrated forward gains them empty.
const ADDED_AFTER_V2: [&str; 9] = [
    "user_mcp_servers",
    "user_machines",
    "box_machines",
    "user_mcp_oauth",
    "mcp_oauth_pending",
    "user_machines_v2",
    "machine_links",
    "user_profiles",
    "box_profiles",
];

fn tables(conn: &Connection) -> Vec<(String, i64)> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    names
        .into_iter()
        .map(|t| {
            let n = conn
                .query_row(&format!("SELECT count(*) FROM \"{t}\""), [], |r| r.get(0))
                .unwrap();
            (t, n)
        })
        .collect()
}

#[test]
fn a_file_from_the_last_build_before_migrations_is_adopted_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = pre_migration_file(dir.path());
    let mut before = tables(&Connection::open(&path).unwrap());
    before.retain(|(t, _)| t != "link_codes");
    super::refuse_other_version(&path).unwrap();
    let mut conn = Connection::open(&path).unwrap();
    init_schema(&mut conn, &path).unwrap();
    let mut after = tables(&conn);
    let history = after
        .iter()
        .position(|(t, _)| t == "refinery_schema_history");
    assert_eq!(
        history.map(|i| after.remove(i)),
        Some((
            "refinery_schema_history".into(),
            i64::try_from(super::schema().unwrap().len()).unwrap()
        ))
    );
    after.retain(|(t, _)| !ADDED_AFTER_V2.contains(&t.as_str()));
    assert_eq!(after, before, "every table and row kept");
    assert_eq!(version(&conn), "16");
}

#[test]
fn applied_migrations_are_never_edited() {
    let pinned: Vec<(i32, u64)> = super::schema()
        .unwrap()
        .iter()
        .map(|m| (m.version(), m.checksum()))
        .collect();
    assert_eq!(
        pinned,
        [
            (1, 7_584_322_431_300_515_816),
            (2, 72_415_543_002_251_104),
            (3, 5_219_232_375_637_337_112),
            (4, 10_900_489_595_431_863_429),
            (5, 5_318_751_723_043_880_253),
            (6, 2_440_567_841_224_804_303),
            (7, 8_758_016_023_718_617_059),
            (8, 1_332_927_781_846_038_899),
            (9, 1_889_085_957_596_602_796),
            (10, 13_545_173_366_957_201_799),
            (11, 17_284_506_714_882_587_087),
            (12, 13_829_219_089_020_947_795),
            (13, 13_975_279_521_182_795_894),
            (14, 14_226_300_463_046_355_811),
            (15, 16_361_376_529_293_542_384),
            (16, 7_650_188_887_765_728_129),
            (17, 13_538_617_703_434_394_357),
            (18, 6_393_984_232_945_565_868),
            (19, 17_463_361_039_883_431_822),
            (20, 17_939_743_571_477_548_879),
            (21, 11_768_997_959_363_115_407),
            (22, 7_963_545_269_253_560_683),
            (23, 3_791_705_829_165_739_481),
            (24, 6_792_033_737_033_784_812),
        ],
        "a schema change is a new migration file; an applied one must not change"
    );
}

#[test]
fn a_file_at_migration_2_gains_user_mcp_servers_and_keeps_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let mut conn = Connection::open(&path).unwrap();
    let all = super::schema().unwrap();
    let up_to_2 = all.get(..2).unwrap();
    moor_migrate::migrate(&mut conn, up_to_2, |_| Ok(moor_migrate::Legacy::Empty)).unwrap();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (7, 1);
         INSERT INTO user_secrets (user, name, value, updated_at) VALUES (7, 'claude', x'00', 1);",
    )
    .unwrap();
    let before = tables(&conn);
    init_schema(&mut conn, &path).unwrap();
    let mut after = tables(&conn);
    assert!(after.contains(&("user_mcp_servers".into(), 0)), "{after:?}");
    after.retain(|(t, _)| !ADDED_AFTER_V2.contains(&t.as_str()) && t != "refinery_schema_history");
    let mut before = before;
    before.retain(|(t, _)| {
        !ADDED_AFTER_V2.contains(&t.as_str()) && t != "refinery_schema_history" && t != "link_codes"
    });
    assert_eq!(after, before, "every table and row kept");
    let applied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM refinery_schema_history WHERE version = 3",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(applied, 1);
}

#[test]
fn bindings_from_before_history_survive_and_a_key_takes_more_sessions() {
    let mut conn = Connection::open_in_memory().unwrap();
    let schema = super::schema().unwrap();
    let before: Vec<_> = schema.iter().take(2).cloned().collect();
    moor_migrate::migrate(&mut conn, &before, super::legacy).unwrap();
    conn.execute_batch(
        "INSERT INTO sessions (id, ordinal, state, started_at, workdir) VALUES \
           (1, 0, 'ended', 1, '/a'), (2, 0, 'live', 2, '/b'), (3, 0, 'live', 3, '/c'); \
         INSERT INTO bindings VALUES ('slack', 't1', 1, 'U1', 10); \
         INSERT INTO bindings VALUES ('web', 'c1', 2, 'U2', 20);",
    )
    .unwrap();
    init_schema(&mut conn, Path::new("moor.db")).unwrap();
    let rows = |conn: &Connection| -> Vec<(String, String, i64, String, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT front, conv_key, session, owner, created_at FROM bindings ORDER BY rowid",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    };
    assert_eq!(
        rows(&conn),
        [
            ("slack".into(), "t1".into(), 1, "U1".into(), 10),
            ("web".into(), "c1".into(), 2, "U2".into(), 20)
        ]
    );
    conn.execute(
        "INSERT INTO bindings VALUES ('slack', 't1', 3, 'U1', 30)",
        [],
    )
    .unwrap();
    assert_eq!(rows(&conn).len(), 3, "a key holds one row per Session");
    let index: i64 = count(
        &conn,
        "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'bindings_session'",
    );
    assert_eq!(index, 1);
}

#[test]
fn a_file_at_migration_4_gains_user_machines_and_keeps_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let mut conn = Connection::open(&path).unwrap();
    let all = super::schema().unwrap();
    let up_to_4 = all.get(..4).unwrap();
    moor_migrate::migrate(&mut conn, up_to_4, |_| Ok(moor_migrate::Legacy::Empty)).unwrap();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (7, 1);
         INSERT INTO user_secrets (user, name, value, updated_at) VALUES (7, 'claude', x'00', 1);",
    )
    .unwrap();
    let mut before = tables(&conn);
    before.retain(|(t, _)| {
        !ADDED_AFTER_V2.contains(&t.as_str()) && t != "refinery_schema_history" && t != "link_codes"
    });
    init_schema(&mut conn, &path).unwrap();
    let mut after = tables(&conn);
    assert!(after.contains(&("user_machines".into(), 0)), "{after:?}");
    after.retain(|(t, _)| !ADDED_AFTER_V2.contains(&t.as_str()) && t != "refinery_schema_history");
    assert_eq!(after, before, "every table and row kept");
}

#[test]
fn a_file_at_migration_8_keeps_its_held_machines_and_lets_asleep_ones_share_a_slot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let mut conn = Connection::open(&path).unwrap();
    let all = super::schema().unwrap();
    let up_to_8 = all.get(..8).unwrap();
    moor_migrate::migrate(&mut conn, up_to_8, |_| Ok(moor_migrate::Legacy::Empty)).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn.execute(
        "INSERT INTO box_machines (box, user, machine, ports, slot) VALUES (1, 7, 'mini', '[8081]', 0)",
        [],
    )
    .unwrap();

    let up_to_9 = all.get(..9).unwrap();
    moor_migrate::migrate(&mut conn, up_to_9, |_| Ok(moor_migrate::Legacy::Empty)).unwrap();

    let asleep = "SELECT COUNT(*) FROM box_machines WHERE box = 1 AND asleep = 0";
    assert_eq!(
        count(&conn, asleep),
        1,
        "an existing row still holds its slot"
    );
    let row = |b: i64, asleep: i64| {
        conn.execute(
            "INSERT INTO box_machines (box, user, machine, ports, slot, asleep) \
             VALUES (?, 7, 'mini', '[8081]', 0, ?)",
            [b, asleep],
        )
    };
    row(2, 1).unwrap();
    row(3, 1).unwrap();
    assert!(row(4, 0).is_err(), "two holding rows never share a slot");
}

#[test]
fn a_file_at_migration_10_turns_asleep_rows_asleep_and_the_rest_held() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let mut conn = Connection::open(&path).unwrap();
    let all = super::schema().unwrap();
    let up_to_10 = all.get(..10).unwrap();
    moor_migrate::migrate(&mut conn, up_to_10, |_| Ok(moor_migrate::Legacy::Empty)).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO box_machines (box, user, machine, ports, slot, sync_exclude, asleep) \
           VALUES (1, 7, 'mini', '[8081]', 0, '[\"target\"]', 0);
         INSERT INTO box_machines (box, user, machine, ports, slot, asleep) \
           VALUES (2, 7, 'mini', '[8091]', 1, 1);",
    )
    .unwrap();

    init_schema(&mut conn, &path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();

    let rows = claim_rows(&conn);
    let held = (
        1,
        "held".into(),
        "mini".into(),
        0,
        "[8081]".into(),
        "[\"target\"]".into(),
        None,
    );
    let asleep = (
        2,
        "asleep".into(),
        "mini".into(),
        1,
        "[8091]".into(),
        "[]".into(),
        None,
    );
    assert_eq!(rows, [held, asleep]);
    let insert = |b: i64, state: &str| {
        conn.execute(
            "INSERT INTO box_machines (box, user, state, machine, ports, slot) \
             VALUES (?, 7, ?, 'mini', '[8081]', 0)",
            rusqlite::params![b, state],
        )
    };
    insert(3, "asleep").unwrap();
    insert(4, "wanted").unwrap();
    assert!(
        insert(5, "held").is_err(),
        "two held rows never share a slot"
    );
    assert!(insert(6, "gone").is_err(), "only the three states");
}

/// A `box_machines` row: box, state, machine, slot, ports, sync excludes, until.
type ClaimRow = (i64, String, String, u16, String, String, Option<i64>);

fn claim_rows(conn: &Connection) -> Vec<ClaimRow> {
    let sql = "SELECT box, state, machine, slot, ports, sync_exclude, until FROM box_machines \
               ORDER BY box";
    let mut stmt = conn.prepare(sql).unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}
