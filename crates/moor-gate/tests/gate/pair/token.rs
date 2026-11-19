//! Pairing by a portal-minted token: the agent's body carries it and the machine is enrolled as
//! the token's owner at once, with no confirm page.

use serde_json::{json, Value};

use super::{assert_enrolled_once_as_alice, assert_handed_out, world, World, KEY};

/// The controller keeps a grant of `token` for alice, as the portal's mint does.
fn grant(w: &World, token: &str) {
    let op = json!({"do": "grant", "ttl_secs": 600, "token": token,
        "owner": {"identity": "authgear:alice-sub", "email": "alice@example.com"}});
    let kept = w.pairings.lock().unwrap().step(&op);
    assert_eq!(kept, json!({"pairing": "started"}));
}

/// The body an agent posts for machine `hv`, with `token` when it has one.
fn body(token: Option<&Value>) -> Value {
    let mut body = json!({"pubkey": KEY, "name": "hv", "slots": 4, "pool": "cloudhypervisor"});
    if let Some(token) = token {
        body["token"] = token.clone();
    }
    body
}

#[test]
fn a_token_enrolls_the_machine_as_its_owner_once() {
    let w = world(true);
    grant(&w, "tok-1");
    let (status, answer) = w.post_json("/machines/pair", &body(Some(&json!("tok-1"))));
    assert_eq!(status, 200, "{answer}");
    let enrolled = json!({"machine_id": 7, "name": "hv", "user": 1, "owner": "alice@example.com",
        "capacity": 4});
    assert_eq!(answer, json!({ "enrolled": enrolled }));
    assert_enrolled_once_as_alice(&w);
    let sent = w.enrolled.lock().unwrap()[0].to_string();
    assert!(
        !sent.contains("tok-1"),
        "the token stays out of the enroll: {sent}"
    );

    let (status, again) = w.post_json("/machines/pair", &body(Some(&json!("tok-1"))));
    assert_eq!(status, 400, "{again}");
    let why = again["error"].as_str().unwrap();
    assert!(why.contains("unknown, used or expired"), "{why}");
    assert_eq!(w.enrolled.lock().unwrap().len(), 1);
}

#[test]
fn an_unknown_or_replaced_token_enrolls_nothing() {
    let w = world(true);
    let (status, _) = w.post_json("/machines/pair", &body(Some(&json!("nope"))));
    assert_eq!(status, 400);
    grant(&w, "old");
    grant(&w, "new");
    let (status, _) = w.post_json("/machines/pair", &body(Some(&json!("old"))));
    assert_eq!(status, 400, "a new grant replaces the owner's earlier one");
    assert!(w.enrolled.lock().unwrap().is_empty());
    assert_eq!(
        w.post_json("/machines/pair", &body(Some(&json!("new")))).0,
        200
    );
}

#[test]
fn a_token_that_is_not_a_string_is_a_bad_request() {
    let w = world(true);
    let (status, answer) = w.post_json("/machines/pair", &body(Some(&json!(7))));
    assert_eq!(status, 400, "{answer}");
}

#[test]
fn a_pairing_without_a_token_still_gets_a_code_to_confirm() {
    let w = world(true);
    grant(&w, "tok-1");
    let asked = w.ask();
    assert_handed_out(&w, &asked);
    assert!(w.enrolled.lock().unwrap().is_empty());
    let (status, answer) = w.post_json("/machines/pair", &body(Some(&json!(null))));
    assert_eq!(status, 200, "a null token is no token: {answer}");
    assert!(answer["user_code"].is_string());
}
