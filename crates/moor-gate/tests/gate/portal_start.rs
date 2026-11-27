//! A send from the portal starts a task in an agent place: an ask, never a Box start.

use serde_json::json;

use super::portal::{csrf_of, json_of, post_to, user};
use super::secrets::world;

#[test]
fn a_send_starts_a_task_in_a_place_and_a_refusal_is_readable() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-ask");
    let csrf = csrf_of(&w, &cookie);
    let body = json!({"repos": ["acme/web", "acme/lib"], "prompt": "Where is GET /items handled?"})
        .to_string();
    let reply = post_to(&w, "/account/api/boxes", &cookie, Some(&csrf), &body);
    assert_eq!(reply.status, 400, "{}", reply.body);
    let why = json_of(&reply)["error"].as_str().unwrap().to_owned();
    assert!(why.contains("claude, github"), "{why}");
    let seen = w.seen.lock().unwrap().clone();
    let ask = seen
        .iter()
        .find(|r| r["request"]["cmd"] == "ask")
        .expect("an ask");
    assert_eq!(
        ask["request"],
        json!({"cmd": "ask", "repos": ["acme/web", "acme/lib"],
            "prompt": "Where is GET /items handled?"})
    );
    assert!(seen.iter().all(|r| r["request"]["cmd"] != "start"));
}
