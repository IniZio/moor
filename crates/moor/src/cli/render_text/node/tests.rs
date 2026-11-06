use moor_core::Response;
use serde_json::{json, Value};

use super::super::render;

#[test]
fn node_ls_shows_warm_starts_on_or_off_with_the_reason_only_when_an_agent_said() {
    let node = |warm: Value| {
        let n = json!({"name": "mini", "scope": "personal", "runtime": "external",
            "online": true, "used": 0, "waiting": 0, "labels": [], "warm_starts": warm});
        let list = Response::ok("node-list", Some(json!({"nodes": [n]})));
        render(&list, false).0
    };
    assert!(node(json!({"state": "on"})).contains("  warm starts on"));
    let off = node(json!({"state": "off", "why": "needs v52.0-moor.1"}));
    assert!(
        off.contains("  warm starts off: needs v52.0-moor.1"),
        "{off}"
    );
    assert!(!node(Value::Null).contains("warm starts"));
}
