//! `V77__launch_facts` fills each recent launch's cause, each live launch's hands and agent facts,
//! each open Box's owed note and cap record, and each tunnel's last restart from the events, on
//! both backends; a launch older than the cap window keeps no cause.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// The fixture on `backend`, with `@T` a time a minute ago, in ms, and `@OLD` one an hour ago, as
/// the database's clock says, since the step bounds by that clock.
/// Box 1 stopped mid-turn, was relaunched, capped, and its live launch stalled, lost its link
/// and saved at exit; Box 2 lost its disk after its last launch ended and relaunched after its cap
/// was recorded and its tunnel restarted an hour ago; Box 3 was relaunched an hour ago and is
/// closed. Box 1's live hands woke a minute ago.
fn before(backend: Backend) -> String {
    let now = match backend {
        Backend::Sqlite => "(CAST(strftime('%s', 'now') AS INTEGER) * 1000)",
        Backend::Postgres => "(CAST(EXTRACT(EPOCH FROM now()) AS BIGINT) * 1000)",
    };
    let t = format!("({now} - 60000)");
    let old = format!("({now} - 3660000)");
    r#"
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO boxes (name, created_at) VALUES ('b', 1);
INSERT INTO boxes (name, created_at) VALUES ('c', 1);
INSERT INTO box_state (box, state, updated_at, resume_first_at) VALUES (1, 'ready', 1, 50);
INSERT INTO box_state (box, state, updated_at) VALUES (2, 'ready', 1);
INSERT INTO box_state (box, state, updated_at, resume_first_at) VALUES (3, 'closed', 1, 9);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (1, 1, 'p', 'gone', 5, 5);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (1, 2, 'p', 'ready', @T + 2, @T + 2);
INSERT INTO launches (box, session_ref, started_at, ended_at) VALUES (1, 's', @T, @T + 1);
INSERT INTO launches (box, session_ref, started_at) VALUES (1, 's', @T + 3);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'relaunched', '{"cause":null}', @T);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'relaunched', '{"cause":"exit"}', @T + 4);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'agent_stalled', '{}', @T + 5);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'agent_resumed', '{}', @T + 6);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'agent_stalled', '{}', @T + 7);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'agent_unreachable', '{}', @T + 8);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'agent_link_restored', '{}', @T + 9);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'agent_unreachable', '{}', @T + 9);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'agent_unreachable', '{}', @T + 10);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'transcript_saved', '{"trigger":"turn"}', @T + 10);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'transcript_saved', '{"trigger":"exit"}', @T + 11);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'relaunch_capped', '{}', @T + 12);
INSERT INTO events (box, kind, payload, ts)
    VALUES (1, 'asleep', '{"reason":"its machine stopped, as when its host restarts"}', @T + 13);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'tunnel_restarted', '{}', @T + 14);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'woke', '{}', @T + 15);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'tunnel_restarted', '{}', @OLD);
INSERT INTO proxy (box, box_port, egress_port, updated_at) VALUES (1, 20001, 10001, 1);
INSERT INTO proxy (box, box_port, egress_port, updated_at) VALUES (2, 20002, 10002, 1);
INSERT INTO launches (box, session_ref, started_at, ended_at) VALUES (2, 's', @T, @T + 1);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'session_started', '{}', @T);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'relaunch_capped', '{}', @T + 1);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'relaunched', '{"cause":"exit"}', @T + 2);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'disk_lost', '{}', @T + 3);
INSERT INTO launches (box, session_ref, started_at, ended_at) VALUES (3, 's', @OLD, @OLD + 1);
INSERT INTO events (box, kind, payload, ts) VALUES (3, 'relaunched', '{"cause":"exit"}', @OLD);
"#
    .replace("@OLD", &old)
    .replace("@T", &t)
}

/// The new `box_state` columns by Box, `-` for null.
const STATE: &str = "SELECT CAST(box AS TEXT) || '|' || COALESCE(owed_note, '-') || '|' \
    || CASE WHEN relaunch_capped_at IS NULL THEN '-' ELSE 'capped' END FROM box_state ORDER BY box";

/// The new `launches` columns by launch, times as set or not.
const LAUNCHES: &str = "SELECT CAST(id AS TEXT) || '|' || COALESCE(cause, '-') || '|' \
    || COALESCE(CAST(hands_inc AS TEXT), '-') || '|' \
    || CASE WHEN stalled_at IS NULL THEN '-' ELSE 'stalled' END || '|' \
    || CASE WHEN unreachable_since IS NULL THEN '-' ELSE 'unreachable' END || '|' \
    || CASE WHEN exit_save_at IS NULL THEN '-' ELSE 'saved' END FROM launches ORDER BY id";

/// Each tunnel's last restart, as set or not.
const PROXY: &str = "SELECT CAST(box AS TEXT) || '|' \
    || CASE WHEN restarted_at IS NULL THEN '-' ELSE 'restarted' END FROM proxy ORDER BY box";

/// Each hands row's last wake, as set or not.
const HANDS: &str = "SELECT CAST(box AS TEXT) || '|' || CAST(inc AS TEXT) || '|' \
    || CASE WHEN woke_at IS NULL THEN '-' ELSE 'woke' END FROM box_hands ORDER BY box, inc";

/// What the step must leave: the unreachable stretch begins after the restore, so its start is
/// checked apart.
fn expected() -> Vec<Vec<&'static str>> {
    vec![
        vec![
            r#"1|{"kind":"suspended","at":50,"stopped":true}|capped"#,
            r#"2|{"kind":"disk_lost"}|-"#,
            "3|-|-",
        ],
        vec![
            "1|-|-|-|-|-",
            "2|exit|2|stalled|unreachable|saved",
            "3|exit|-|-|-|-",
            "4|-|-|-|-|-",
        ],
        vec!["1|restarted", "2|-"],
        vec!["1|1|-", "1|2|woke"],
    ]
}

/// When launch 2 first went unreachable, minus the fixture's `@T`.
const UNREACHABLE: &str =
    "SELECT unreachable_since - (SELECT started_at FROM launches WHERE id = 1) \
    FROM launches WHERE id = 2";

#[test]
fn sqlite_fills_launch_and_box_facts_from_the_events() {
    let (before_sql, step) = super::before_and_step("V77__launch_facts", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before_sql).unwrap();
    conn.execute_batch(&before(Backend::Sqlite)).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let got: Vec<Vec<String>> = [STATE, LAUNCHES, PROXY, HANDS]
        .iter()
        .map(|q| {
            let mut stmt = conn.prepare(q).unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        })
        .collect();
    assert_eq!(got, expected());
    let since: i64 = conn.query_row(UNREACHABLE, [], |r| r.get(0)).unwrap();
    assert_eq!(since, 9);
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_fills_launch_and_box_facts_from_the_events() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before_sql, step) = super::before_and_step("V77__launch_facts", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_launch_facts_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<(Vec<Vec<String>>, i64), postgres::Error> {
        client.batch_execute(&before_sql)?;
        client.batch_execute(&before(Backend::Postgres))?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        let mut got = Vec::new();
        for q in [STATE, LAUNCHES, PROXY, HANDS] {
            got.push(client.query(q, &[])?.iter().map(|r| r.get(0)).collect());
        }
        let since: i64 = client.query_one(UNREACHABLE, &[])?.get(0);
        Ok((got, since))
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    let (got, since) = got.unwrap();
    assert_eq!(got, expected());
    assert_eq!(since, 9);
}
