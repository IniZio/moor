//! Read-only views of the new binary's SQLite store (events, sessions).
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::adapter::{Event, EventKind, Session};
use crate::world::World;

fn open(w: &World) -> Option<Connection> {
    let db = w.state_dir().join("moor.db");
    db.exists().then(|| {
        Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("open {}: {e}", db.display()))
    })
}

pub(super) fn events(w: &World, name: &str) -> Vec<Event> {
    let Some(conn) = open(w) else {
        return Vec::new();
    };
    let mut stmt = conn
        .prepare(
            "SELECT e.kind, e.payload FROM events e JOIN tasks t ON t.id = e.task \
             WHERE t.name = ?1 ORDER BY e.id",
        )
        .expect("prepare events query");
    let rows = stmt
        .query_map([name], |r| {
            let kind: String = r.get(0)?;
            let payload: String = r.get(1)?;
            Ok(Event {
                kind: EventKind::from_old(&kind),
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            })
        })
        .expect("query events");
    rows.map(|r| r.expect("event row")).collect()
}

pub(super) fn sessions(w: &World, name: &str) -> Vec<Session> {
    let Some(conn) = open(w) else {
        return Vec::new();
    };
    let mut stmt = conn
        .prepare(
            "SELECT s.session_ref, s.ended_at IS NULL FROM sessions s \
             JOIN tasks t ON t.id = s.task WHERE t.name = ?1 ORDER BY s.id",
        )
        .expect("prepare sessions query");
    let rows = stmt
        .query_map([name], |r| {
            Ok(Session {
                session_id: r.get(0)?,
                box_pane: true,
                live: r.get(1)?,
            })
        })
        .expect("query sessions");
    rows.map(|r| r.expect("session row")).collect()
}
