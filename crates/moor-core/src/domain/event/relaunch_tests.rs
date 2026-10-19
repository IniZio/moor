//! Tests for the relaunch audit event payload.

use super::*;

fn s(v: &str) -> String {
    v.to_owned()
}

#[test]
fn relaunched_cause_is_omitted_when_none() {
    let e = Event::Relaunched {
        old_pane: None,
        new_pane: None,
        goal: None,
        cause: None,
    };
    assert!(e.payload().get("cause").is_none());
}

#[test]
fn relaunched_cause_is_in_payload_when_set() {
    let e = Event::Relaunched {
        old_pane: None,
        new_pane: None,
        goal: None,
        cause: Some(s("wake")),
    };
    assert_eq!(e.payload().get("cause"), Some(&serde_json::json!("wake")));
    assert_eq!(e.kind(), "relaunched");
}
