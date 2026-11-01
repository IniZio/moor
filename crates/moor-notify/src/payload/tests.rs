use moor_client::EventRecord;
use moor_core::{BoxId, SessionId};
use serde_json::json;

use super::body;

/// An `idled` event of Box `app/fix` with id 7.
fn idled() -> EventRecord {
    EventRecord {
        cursor: 120,
        box_name: Some("app/fix".to_owned()),
        box_id: Some(BoxId::new(7)),
        session: Some(SessionId::new(3)),
        kind: "idled".to_owned(),
        payload: json!({}),
        ts: 1_700_000_000_000,
    }
}

#[test]
fn carries_the_event_and_the_box_status_fields() {
    let shown = json!({"box": {
        "name": "app/fix",
        "lifecycle": "ready",
        "session_state": "idle",
        "prompt": "not copied",
        "said": {"text": "PR opened", "at": "2026-11-01T00:00:00Z"},
    }});
    assert_eq!(
        body(&idled(), Some(&shown)),
        json!({
            "schema": "moor.notify/1",
            "delivery": 120,
            "event": "idled",
            "ts": 1_700_000_000_000_i64,
            "box": "app/fix",
            "box_id": 7,
            "session": 3,
            "data": {},
            "lifecycle": "ready",
            "session_state": "idle",
            "agent_said": {"text": "PR opened", "at": "2026-11-01T00:00:00Z"},
        })
    );
}

#[test]
fn without_a_show_it_still_carries_the_event() {
    let mut event = idled();
    event.kind = "setup_failed".to_owned();
    event.payload = json!({"line": "make setup exited 2"});
    let got = body(&event, None);
    assert_eq!(got.pointer("/event"), Some(&json!("setup_failed")));
    assert_eq!(
        got.pointer("/data/line"),
        Some(&json!("make setup exited 2"))
    );
    assert!(got.get("agent_said").is_none());
    assert!(got.get("lifecycle").is_none());
}
