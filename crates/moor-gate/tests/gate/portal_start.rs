//! A send from the portal starts a Box, with the base, machine and plan choice it names.

use serde_json::json;

use super::portal::{csrf_of, post_to, user};
use super::secrets::world;

#[test]
fn a_send_starts_a_box() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-ask");
    let csrf = csrf_of(&w, &cookie);
    let body = json!({"repos": ["acme/web", "acme/lib"], "prompt": "Where is GET /items handled?",
        "base": "main", "node": "engine-03", "plan_first": true})
    .to_string();
    let reply = post_to(&w, "/account/api/boxes", &cookie, Some(&csrf), &body);
    assert_eq!(reply.status, 400, "{}", reply.body);
    let seen = w.seen.lock().unwrap().clone();
    assert!(seen.iter().any(|r| r["request"]["cmd"] == "start"));
    assert!(seen.iter().all(|r| r["request"]["cmd"] != "ask"));
}
