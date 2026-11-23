//! `list_boxes` against the fake controller.

use serde_json::{json, Value};

use super::mcp;

/// The Boxes `show` lists for identity `who`: a full row of the controller's, then two sparse ones.
pub(super) fn boxes(who: &str) -> Value {
    json!({"boxes": [{"name": format!("repo/{}", who.replace(':', "-")),
        "state": "ready", "repo": "acme/web",
        "other_repos": ["acme/api", "acme/docs"],
        "prompt": "a very long prompt",
        "driver": {"state": "ready", "provider": "engine-03"},
        "agent": {"state": "idle"}, "run": {"id": 1},
        "gate": {"gates": ["proof"]},
        "progress": {"pr": {"repo": "acme/web", "number": 212}},
        "updated_at": "2026-10-29T10:00:00Z"},
        {"name": "repo/failed", "lifecycle": "setup_failed"},
        {"name": "repo/starting", "lifecycle": "setting_up"}]})
}

/// The sorted keys of `row`.
fn keys(row: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn list_boxes_rows_are_compact_and_leave_out_the_prompt() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.call(&token, "list_boxes", json!({}));
    let boxes = reply["structuredContent"]["boxes"].as_array().unwrap();
    assert_eq!(
        keys(&boxes[0]),
        [
            "name",
            "node",
            "other_repos",
            "pr",
            "repo",
            "status",
            "updated_at"
        ],
        "{reply}"
    );
    assert_eq!(boxes[0]["repo"], "acme/web");
    assert_eq!(boxes[0]["other_repos"], 2);
    assert_eq!(boxes[0]["node"], "engine-03");
    assert_eq!(boxes[0]["pr"], "acme/web#212");
    assert_eq!(boxes[0]["updated_at"], "2026-10-29T10:00:00Z");
    for row in &boxes[1..] {
        assert_eq!(keys(row), ["name", "node", "repo", "status", "updated_at"]);
    }
    assert!(!reply.to_string().contains("a very long prompt"), "{reply}");
}
