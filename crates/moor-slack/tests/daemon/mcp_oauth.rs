//! `/moor mcp add` with no key against a scripted controller: the form turns into a Sign in
//! link and a Finish sign-in step, and a key entered as before still takes the old path.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::time::Duration;

use moor_core::request::Request;
use serde_json::{json, Value};

use moor_core::response::Response;

use super::support::controller::{failed, refused, FakeController, Scripted};
use super::support::{Rig, CHANNEL, USER};

const LINEAR: &str = "https://mcp.linear.app/mcp";
const AUTHORIZE: &str = "https://auth.example.com/authorize?client_id=c1&state=st-1";
const REDIRECT: &str =
    "http://127.0.0.1:1/callback?code=code-9&state=st-1&iss=https%3A%2F%2Fauth.example.com";
const WAIT: Duration = Duration::from_secs(10);

fn added(sign_in: Option<&str>) -> Scripted {
    let value =
        json!({"name": "linear", "changed": true, "secret": "mcp_linear", "sign_in": sign_in});
    Scripted::Reply(Response::ok("mcp-add", Some(value)))
}

fn open_modal(rig: &Rig) -> Value {
    rig.slack.send_slash("env-1", CHANNEL, USER, "mcp add");
    rig.slack.wait_views("views.open", 1, WAIT)[0]["view"].clone()
}

fn submit(rig: &Rig, envelope: &str, view: &Value, fields: &[(&str, &str)]) -> Value {
    rig.slack.send_fields(envelope, USER, view, fields);
    rig.slack.wait_ack_payload(envelope)
}

/// Adds Linear with an empty key; returns the view the ack shows.
fn add_without_key(rig: &Rig) -> Value {
    let view = open_modal(rig);
    let fields = [("name", "linear"), ("url", LINEAR), ("key", "")];
    let ack = submit(rig, "env-2", &view, &fields);
    assert_eq!(ack["response_action"], "update", "{ack}");
    ack["view"].clone()
}

fn finishes(controller: &FakeController) -> Vec<Request> {
    controller.seen(|seen| {
        let finish = |r: &&Request| matches!(r, Request::McpOAuthFinish(_));
        seen.requests.iter().filter(finish).cloned().collect()
    })
}

#[test]
fn an_empty_key_shows_a_sign_in_link_button_to_that_user_only() {
    let (rig, controller) = Rig::start();
    controller.script("mcp-add", added(Some(AUTHORIZE)));
    let view = add_without_key(&rig);
    let button = view["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| {
            b["elements"]
                .as_array()
                .into_iter()
                .flatten()
                .chain(b.get("accessory"))
        })
        .find(|e| e["type"] == "button")
        .cloned()
        .expect("a button");
    assert_eq!(button["url"], AUTHORIZE);
    assert_eq!(button["text"]["text"], "Sign in");
    let shown = view.to_string();
    assert!(shown.contains(
        "After you approve, if the page doesn't load, copy its address and use Finish sign-in."
    ));
    assert_eq!(view["submit"]["text"], "Finish sign-in");
    let adds = controller.seen(|s| {
        s.requests
            .iter()
            .filter_map(|r| match r {
                Request::McpAdd(add) => Some(add.key.is_none()),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(adds, [true]);
}

#[test]
fn finish_sends_the_pasted_address_and_says_connected() {
    let (rig, controller) = Rig::start();
    controller.script("mcp-add", added(Some(AUTHORIZE)));
    controller.script(
        "mcp-oauth-finish",
        Scripted::Reply(Response::ok(
            "mcp-oauth-finish",
            Some(json!({"name": "linear"})),
        )),
    );
    let view = add_without_key(&rig);
    let ack = submit(&rig, "env-3", &view, &[("address", REDIRECT)]);
    assert_eq!(ack["response_action"], "update", "{ack}");
    assert!(
        ack["view"].to_string().contains("linear connected"),
        "{ack}"
    );
    let sent = finishes(&controller);
    let [Request::McpOAuthFinish(finish)] = &sent[..] else {
        panic!("{sent:?}")
    };
    assert_eq!(finish.state, "st-1");
    assert_eq!(finish.code, "code-9");
    assert_eq!(finish.iss.as_deref(), Some("https://auth.example.com"));
}

#[test]
fn a_refused_finish_shows_on_the_address_field() {
    let (rig, controller) = Rig::start();
    controller.script("mcp-add", added(Some(AUTHORIZE)));
    controller.script(
        "mcp-oauth-finish",
        Scripted::Reply(refused("mcp-oauth-finish", "this sign-in expired")),
    );
    let view = add_without_key(&rig);
    let ack = submit(&rig, "env-3", &view, &[("address", REDIRECT)]);
    assert_eq!(ack["response_action"], "errors", "{ack}");
    assert!(
        ack["errors"]["address"]
            .as_str()
            .unwrap()
            .contains("expired"),
        "{ack}"
    );
}

#[test]
fn an_address_without_a_code_is_refused_before_the_controller() {
    let (rig, controller) = Rig::start();
    controller.script("mcp-add", added(Some(AUTHORIZE)));
    controller.script(
        "mcp-oauth-finish",
        Scripted::Reply(failed("mcp-oauth-finish", "unreachable")),
    );
    let view = add_without_key(&rig);
    let ack = submit(&rig, "env-3", &view, &[("address", "not an address")]);
    assert_eq!(ack["response_action"], "errors", "{ack}");
    assert!(ack["errors"]["address"].is_string(), "{ack}");
    assert!(finishes(&controller).is_empty());
}

#[test]
fn a_key_goes_down_the_old_path() {
    let (rig, controller) = Rig::start();
    controller.script("mcp-add", added(None));
    let view = open_modal(&rig);
    let fields = [("name", "linear"), ("url", LINEAR), ("key", "lin_api_fake")];
    let ack = submit(&rig, "env-2", &view, &fields);
    assert_eq!(ack["response_action"], "update", "{ack}");
    let shown = ack["view"].to_string();
    assert!(
        shown.contains("Added `linear`. Its key is in your vault."),
        "{shown}"
    );
    assert!(!shown.contains("Sign in"), "{shown}");
}
