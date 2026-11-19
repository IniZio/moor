//! Marking a stale plan reviewed from the workflow page.

use serde_json::{json, Value};

use super::{as_user, json_of, plan, put, world};

#[test]
fn marking_a_plan_reviewed_drops_made_from_and_is_not_an_edit() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-61");
    let mut reviewed = plan();
    reviewed.as_object_mut().unwrap().remove("made_from");
    let body = json!({"prompt": "same prompt", "plan": reviewed.to_string(), "revision": 3});
    let reply = put(&w, &cookie, &body);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(json_of(&reply)["plan_edited"], false);
    let seen = w.seen.lock().unwrap().clone();
    let add = seen
        .iter()
        .find(|r| r["request"]["cmd"] == "workflow-add")
        .unwrap();
    let sent: Value = serde_json::from_str(add["request"]["plan"].as_str().unwrap()).unwrap();
    assert!(sent.get("made_from").is_none(), "{sent}");
    assert_eq!(sent["edited"], false);
}
