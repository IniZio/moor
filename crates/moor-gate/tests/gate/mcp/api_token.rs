//! `moor_` API tokens on `/mcp`, resolved by the fake controller.

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::{mcp, mpsc};

/// A token the fake controller resolves to a person with read scope only.
const READ_TOKEN: &str = "moor_rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr";

/// A token the fake controller knows nothing of.
const UNKNOWN_TOKEN: &str = "moor_uuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuu";

/// The fake controller's answer to an `api-token-resolve`, if `frame` is one.
pub(super) fn resolved(frame: &Value) -> Option<Value> {
    let request = &frame["request"];
    if request["cmd"] != "api-token-resolve" {
        return None;
    }
    let envelope = if request["token"] == READ_TOKEN {
        let value = json!({"identity": "authgear:bot-sub", "email": "bot@example.com",
            "scopes": ["moor:read"]});
        Response::ok("api-token-resolve", Some(value))
    } else {
        let error = ErrorView {
            kind: ErrorKind::Usage,
            message: "unknown API token".to_owned(),
            hints: vec![],
            context: vec![],
            detail: None,
        };
        Response::err("api-token-resolve", error)
    };
    Some(json!({"frame": "response", "id": frame["id"], "envelope": envelope}))
}

fn is_resolve(frame: &Value) -> bool {
    frame["request"]["cmd"] == "api-token-resolve"
}

#[test]
fn an_api_token_acts_as_its_person() {
    let m = mcp();
    let result = m.call(READ_TOKEN, "list_boxes", json!({}));
    assert_ne!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("repo/authgear-bot-sub"), "{text}");
    let sent: Vec<Value> = m
        .requests()
        .into_iter()
        .filter(|f| !is_resolve(f))
        .collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["request"]["identity"], "authgear:bot-sub");
}

#[test]
fn a_read_api_token_cannot_start_a_box_or_add_a_webhook() {
    let m = mcp();
    for (tool, args) in [
        ("start_box", json!({"repo": "acme/web", "prompt": "hi"})),
        (
            "add_webhook",
            json!({"url": "https://hooks.example/moor", "events": ["box_ready"]}),
        ),
    ] {
        let result = m.call(READ_TOKEN, tool, args);
        assert_eq!(result["isError"], true, "{tool}: {result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("moor:write"), "{tool}: {text}");
    }
    assert!(m.requests().iter().all(is_resolve));
}

#[test]
fn an_unknown_api_token_gets_the_same_401_as_a_bad_access_token() {
    let m = mcp();
    let (status, _, challenge) = m.post(Some(UNKNOWN_TOKEN), &json!({}), None);
    assert_eq!(status, 401);
    let (_, _, bad_jwt) = m.post(Some("not-a-jwt"), &json!({}), None);
    assert_eq!(challenge, bad_jwt);
    assert!(
        challenge.starts_with("Bearer resource_metadata="),
        "{challenge}"
    );
}

#[test]
fn a_polling_client_resolves_its_token_once() {
    let m = mcp();
    m.call(READ_TOKEN, "list_boxes", json!({}));
    m.call(READ_TOKEN, "list_boxes", json!({}));
    let resolves = m.requests().iter().filter(|f| is_resolve(f)).count();
    assert_eq!(resolves, 1);
}

#[test]
fn an_unreachable_controller_is_503_not_401() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("nobody.sock");
    let (_unused, rx) = mpsc::channel();
    let world = super::world_with(socket, rx, dir, true);
    let status = ureq::Agent::from(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    )
    .post(format!("http://{}/mcp", world.gate))
    .header("Content-Type", "application/json")
    .header("Accept", "application/json, text/event-stream")
    .header("Authorization", format!("Bearer {READ_TOKEN}"))
    .send("{}")
    .unwrap()
    .status()
    .as_u16();
    assert_eq!(status, 503);
}

#[test]
fn front_refuses_an_api_token() {
    let m = mcp();
    assert_eq!(
        super::super::connect(&m.world, Some(READ_TOKEN)).err(),
        Some(401)
    );
}
