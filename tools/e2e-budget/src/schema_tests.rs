//! Queries run against the schema Moor creates.

use super::*;

#[test]
fn queries_run_against_the_real_schema() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(include_str!("../../../crates/moor/src/store/schema.sql"))
        .unwrap();
    conn.execute_batch(
        "INSERT INTO boxes (id, name, repo, state, created_at, updated_at) VALUES (1, 'b', 'r', 'open', 0, 0);
         INSERT INTO events (id, box, kind, payload, ts) VALUES (7, 1, 'box_ready', '{}', 1000);
         INSERT INTO sessions (id, box, ordinal, state, started_at, agent_session_id) VALUES (1, 1, 1, 'live', 0, 'abc');
         INSERT INTO launches (box, session_ref, started_at, registered_agent_session) VALUES (1, 'x', 0, NULL);",
    )
    .unwrap();
    let events = fetch_events(&conn, "b", 0).unwrap();
    assert_eq!(events.len(), 1);
    assert!(fetch_events(&conn, "b", 7).unwrap().is_empty());
    let (launch, session) = agent_session_ids(&conn, "b").unwrap();
    assert_eq!((launch, session.as_deref()), (None, Some("abc")));
}
