//! A send from the portal asks when the controller lists the person for agent places, and
//! starts a Box for everyone else.

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::portal::{csrf_of, json_of, post_to, user};
use super::secrets::world;

/// Accounts whose subject starts with this are on the fake controller's `agent_places` list.
pub(super) const ASKER: &str = "asker-";

/// What the fake controller answers `whoami`.
pub(super) fn whoami(who: &str) -> Value {
    let mut value = json!({"identity": who, "user": 1, "identities": [who]});
    if who.contains(ASKER) {
        value["asks"] = json!(true);
    }
    value
}

/// What the fake controller answers `ask`: a place that took the question, or a refusal naming
/// a repo it cannot read.
pub(super) fn ask(who: &str, inner: &Value) -> Response {
    let nope = inner["repos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r.as_str().is_some_and(|r| r.ends_with("/nope")));
    if let Some(repo) = nope {
        let view = ErrorView {
            kind: ErrorKind::Refused,
            message: format!("you cannot read {}", repo.as_str().unwrap()),
            hints: vec![],
            context: vec![],
            detail: None,
        };
        return Response::err("x", view);
    }
    assert!(who.contains(ASKER), "only a listed person asks");
    if inner["repos"][0] == "acme/cold" {
        let value = json!({"box": "place-5e6f7a8b", "claimed": false, "claimed_at": 1,
            "no_idle_place": "no idle place: place-0a0b0c0d failed: profile: not found; close it with close_box place-0a0b0c0d and moor starts the next place"});
        return Response::ok("x", Some(value));
    }
    let value = json!({"box": "place-1a2b3c4d", "claimed": true, "claimed_at": 1,
        "served_at": 2, "steered_at": 3});
    Response::ok("x", Some(value))
}

fn body() -> String {
    json!({"repos": ["acme/web", "acme/lib"], "prompt": "Where is GET /items handled?",
        "base": "main", "node": "engine-03", "plan_first": true})
    .to_string()
}

#[test]
fn a_roster_person_s_send_asks() {
    let w = world(true, &[]);
    let cookie = user(&w, "asker-ann");
    let csrf = csrf_of(&w, &cookie);
    assert_eq!(
        json_of(&super::login::get(&w.addr, "/account/api", Some(&cookie)))["asks"],
        true
    );
    let reply = post_to(&w, "/account/api/boxes", &cookie, Some(&csrf), &body());
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(json_of(&reply), json!({"box": "place-1a2b3c4d"}));
    let seen = w.seen.lock().unwrap().clone();
    let ask = seen
        .iter()
        .find(|r| r["request"]["cmd"] == "ask")
        .expect("an ask");
    assert_eq!(ask["request"]["repos"], json!(["acme/web", "acme/lib"]));
    assert_eq!(ask["request"]["prompt"], "Where is GET /items handled?");
    assert!(
        seen.iter().all(|r| r["request"]["cmd"] != "start"),
        "no Box starts: {seen:?}"
    );
}

#[test]
fn a_refused_ask_is_readable_on_the_page() {
    let w = world(true, &[]);
    let cookie = user(&w, "asker-bob-refused");
    let csrf = csrf_of(&w, &cookie);
    let sent = json!({"repos": ["acme/nope"], "prompt": "q"}).to_string();
    let reply = post_to(&w, "/account/api/boxes", &cookie, Some(&csrf), &sent);
    assert_eq!(reply.status, 400, "{}", reply.body);
    let why = json_of(&reply)["error"].as_str().unwrap().to_owned();
    assert!(why.contains("cannot read acme/nope"), "{why}");
}

#[test]
fn a_person_off_the_roster_starts_as_before() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-ask");
    let csrf = csrf_of(&w, &cookie);
    let view: Value = json_of(&super::login::get(&w.addr, "/account/api", Some(&cookie)));
    assert!(view.get("asks").is_none_or(|a| a == false), "{view}");
    let reply = post_to(&w, "/account/api/boxes", &cookie, Some(&csrf), &body());
    assert_eq!(reply.status, 400, "{}", reply.body);
    let seen = w.seen.lock().unwrap().clone();
    assert!(seen.iter().any(|r| r["request"]["cmd"] == "start"));
    assert!(seen.iter().all(|r| r["request"]["cmd"] != "ask"));
}

#[test]
fn a_cold_ask_tells_the_page_why_no_idle_place_took_it() {
    let w = world(true, &[]);
    let cookie = user(&w, "asker-cara");
    let csrf = csrf_of(&w, &cookie);
    let sent = json!({"repos": ["acme/cold"], "prompt": "q"}).to_string();
    let reply = post_to(&w, "/account/api/boxes", &cookie, Some(&csrf), &sent);
    assert_eq!(reply.status, 200, "{}", reply.body);
    let view = json_of(&reply);
    assert_eq!(view["box"], "place-5e6f7a8b");
    let why = view["no_idle_place"].as_str().unwrap();
    assert!(why.contains("close_box place-0a0b0c0d"), "{why}");
}
