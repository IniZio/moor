//! `V76__wanted_effect` fills the wanted action, the effect in flight, the owed note, the cap
//! record, each launch's cause, hands and agent facts, and each tunnel's last restart from the
//! intent and events that held them, on both backends.

use moor_migrate::Backend;
use postgres::{Client, NoTls};
use rusqlite::Connection;

/// Box 1 acquires and has asked twice since its start began; Box 2 sets up, asked once since; Box
/// 3 runs run 7; Box 4 is being force-closed; Box 5 stopped mid-turn, was relaunched, capped, and
/// its live launch stalled, lost its link twice and saved at exit; Box 6 lost its disk after its
/// last launch ended, and relaunched after its cap was recorded.
const BEFORE: &str = r#"
INSERT INTO boxes (name, created_at) VALUES ('a', 1);
INSERT INTO boxes (name, created_at) VALUES ('b', 1);
INSERT INTO boxes (name, created_at) VALUES ('c', 1);
INSERT INTO boxes (name, created_at) VALUES ('d', 1);
INSERT INTO boxes (name, created_at) VALUES ('e', 1);
INSERT INTO boxes (name, created_at) VALUES ('f', 1);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_at)
    VALUES (1, 'setting_up', 1, 'provisioning', 9, 100);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_at)
    VALUES (2, 'setting_up', 1, 'setup', 9, 200);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_run, intent_at)
    VALUES (3, 'ready', 1, 'running', 9, 7, 300);
INSERT INTO box_state (box, state, updated_at, intent_kind, intent_pid, intent_at)
    VALUES (4, 'setting_up', 1, 'closing', 9, 400);
INSERT INTO box_state (box, state, updated_at, resume_first_at) VALUES (5, 'ready', 1, 50);
INSERT INTO box_state (box, state, updated_at) VALUES (6, 'ready', 1);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'box_requested', '{}', 1);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'setting_up', '{}', 2);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'box_requested', '{}', 3);
INSERT INTO events (box, kind, payload, ts) VALUES (1, 'box_requested', '{}', 4);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'box_requested', '{}', 1);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'setting_up', '{}', 2);
INSERT INTO events (box, kind, payload, ts) VALUES (2, 'box_requested', '{}', 3);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (5, 1, 'p', 'gone', 25, 5);
INSERT INTO box_hands (box, inc, provider, state, state_at, created_at)
    VALUES (5, 2, 'p', 'ready', 25, 25);
INSERT INTO launches (box, session_ref, started_at, ended_at) VALUES (5, 's', 10, 20);
INSERT INTO launches (box, session_ref, started_at) VALUES (5, 's', 30);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'relaunched', '{"cause":null}', 12);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'agent_unreachable', '{}', 15);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'relaunched', '{"cause":"exit"}', 31);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'agent_stalled', '{}', 32);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'agent_resumed', '{}', 33);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'agent_stalled', '{}', 34);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'agent_unreachable', '{}', 35);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'agent_link_restored', '{}', 36);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'agent_unreachable', '{}', 37);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'agent_unreachable', '{}', 38);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'transcript_saved', '{"trigger":"turn"}', 38);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'transcript_saved', '{"trigger":"exit"}', 39);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'relaunch_capped', '{}', 40);
INSERT INTO events (box, kind, payload, ts)
    VALUES (5, 'asleep', '{"reason":"its machine stopped, as when its host restarts"}', 45);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'tunnel_restarted', '{}', 41);
INSERT INTO events (box, kind, payload, ts) VALUES (5, 'tunnel_restarted', '{}', 42);
INSERT INTO proxy (box, box_port, egress_port, updated_at) VALUES (5, 20005, 10005, 1);
INSERT INTO proxy (box, box_port, egress_port, updated_at) VALUES (6, 20006, 10006, 1);
INSERT INTO launches (box, session_ref, started_at, ended_at) VALUES (6, 's', 10, 20);
INSERT INTO events (box, kind, payload, ts) VALUES (6, 'session_started', '{}', 10);
INSERT INTO events (box, kind, payload, ts) VALUES (6, 'relaunch_capped', '{}', 11);
INSERT INTO events (box, kind, payload, ts) VALUES (6, 'relaunched', '{"cause":"exit"}', 12);
INSERT INTO events (box, kind, payload, ts) VALUES (6, 'disk_lost', '{}', 21);
"#;

/// The new `box_state` columns by Box, `-` for null.
const STATE: &str = "SELECT CAST(box AS TEXT) || '|' || COALESCE(wanted, '-') || '|' \
    || COALESCE(CAST(wanted_force AS TEXT), '-') || '|' || COALESCE(CAST(wanted_at AS TEXT), '-') \
    || '|' || COALESCE(effect, '-') || '|' || COALESCE(CAST(effect_attempt AS TEXT), '-') || '|' \
    || COALESCE(CAST(effect_run AS TEXT), '-') || '|' || COALESCE(CAST(effect_at AS TEXT), '-') \
    || '|' || COALESCE(owed_note, '-') || '|' \
    || COALESCE(CAST(relaunch_capped_at AS TEXT), '-') FROM box_state ORDER BY box";

/// The new `launches` columns by launch, `-` for null.
const LAUNCHES: &str = "SELECT CAST(id AS TEXT) || '|' || COALESCE(cause, '-') || '|' \
    || COALESCE(CAST(hands_inc AS TEXT), '-') || '|' || COALESCE(CAST(stalled_at AS TEXT), '-') \
    || '|' || COALESCE(CAST(unreachable_since AS TEXT), '-') || '|' \
    || COALESCE(CAST(exit_save_at AS TEXT), '-') FROM launches ORDER BY id";

/// Each tunnel's last restart, `-` for none.
const PROXY: &str = "SELECT CAST(box AS TEXT) || '|' || COALESCE(CAST(restarted_at AS TEXT), '-') \
    FROM proxy ORDER BY box";

fn expected() -> Vec<Vec<&'static str>> {
    vec![
        vec![
            "1|-|-|-|acquire|2|-|100|-|-",
            "2|-|-|-|setup|1|-|200|-|-",
            "3|-|-|-|run|1|7|300|-|-",
            "4|close|1|400|-|-|-|-|-|-",
            r#"5|-|-|-|-|-|-|-|{"kind":"suspended","at":50,"stopped":true}|40"#,
            r#"6|-|-|-|-|-|-|-|{"kind":"disk_lost"}|-"#,
        ],
        vec!["1|-|1|-|-|-", "2|exit|2|34|37|39", "3|exit|-|-|-|-"],
        vec!["5|42", "6|-"],
    ]
}

#[test]
fn sqlite_fills_wanted_and_effect_from_intent_and_events() {
    let (before, step) = super::before_and_step("V76__wanted_effect", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    let got: Vec<Vec<String>> = [STATE, LAUNCHES, PROXY]
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
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_fills_wanted_and_effect_from_intent_and_events() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let (before, step) = super::before_and_step("V76__wanted_effect", Backend::Postgres);
    let mut client = Client::connect(&url, NoTls).unwrap();
    let name = format!("moor_wanted_effect_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let mut run = || -> Result<Vec<Vec<String>>, postgres::Error> {
        client.batch_execute(&before)?;
        client.batch_execute(BEFORE)?;
        client.batch_execute(&format!("BEGIN; {step} COMMIT;"))?;
        [STATE, LAUNCHES, PROXY]
            .iter()
            .map(|q| Ok(client.query(*q, &[])?.iter().map(|r| r.get(0)).collect()))
            .collect()
    };
    let got = run();
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    assert_eq!(got.unwrap(), expected());
}
