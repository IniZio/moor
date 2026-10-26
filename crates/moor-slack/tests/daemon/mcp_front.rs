//! `/moor mcp add`, `list` and `remove` against a real controller: each Slack user's servers
//! and keys are their own, and a key is never in anything Slack or the log sees.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::Path;
use std::time::Duration;

use moor_client::Client;
use moor_core::request::{AsReq, Request};
use moor_core::response::{ExitClass, Response};
use moor_core::user::Identity;
use serde_json::Value;

use super::support::slack_fake::TEAM;
use super::support::{Rig, CHANNEL, USER};

/// A made-up key. It is no real credential.
const KEY: &str = "lin_api_fake_key_9d41c7e0a2b86f35";

/// The URL of the Linear read-only server.
const LINEAR: &str = "https://mcp.linear.app/mcp/readonly";

/// A second Slack user.
const OTHER: &str = "U0SECOND";

/// How long a fake step may take.
const WAIT: Duration = Duration::from_secs(10);

fn identity(user: &str) -> Identity {
    Identity::parse(&format!("slack:{TEAM}/{user}")).unwrap()
}

fn ask_as(front: &Path, user: &str, request: Request) -> Response {
    let reply = Client::new(front)
        .acting_as(identity(user))
        .request(request, |_| {})
        .unwrap();
    assert_eq!(reply.class, ExitClass::Ok, "{reply:?}");
    reply
}

/// The servers `user` has, as `(name, url, has_key)`, asked of the controller itself.
fn servers(front: &Path, user: &str) -> Vec<(String, String, bool)> {
    let listed = ask_as(front, user, Request::McpList(AsReq { identity: None }));
    let servers = listed.value.unwrap()["servers"].clone();
    servers
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let text = |k: &str| s[k].as_str().unwrap().to_owned();
            (text("name"), text("url"), s["has_key"].as_bool().unwrap())
        })
        .collect()
}

fn rig() -> (Rig, super::support::RealController) {
    let mut rig = Rig::new();
    let controller = rig.real_controller();
    let front = rig.front_socket();
    for user in [USER, OTHER] {
        let enroll = Request::Enroll(AsReq { identity: None });
        ask_as(&front, user, enroll);
    }
    rig.spawn_daemon_on_front();
    rig.slack.wait_connected();
    (rig, controller)
}

/// Runs `/moor mcp add` as `user`; returns the modal it opened.
fn open_modal(rig: &Rig, envelope: &str, user: &str) -> Value {
    rig.slack.send_slash(envelope, CHANNEL, user, "mcp add");
    let opened = rig.slack.wait_views("views.open", 1, WAIT);
    assert_eq!(opened[0]["trigger_id"], format!("trigger-{envelope}"));
    opened[0]["view"].clone()
}

/// Submits the modal as `user` with these fields; returns the ack's payload.
fn submit(rig: &Rig, envelope: &str, user: &str, view: &Value, fields: &[(&str, &str)]) -> Value {
    rig.slack.send_fields(envelope, user, view, fields);
    rig.slack.wait_ack_payload(envelope)
}

/// Adds the Linear server with [`KEY`] as `user`.
fn add_linear(rig: &Rig, user: &str) {
    let view = open_modal(rig, "env-1", user);
    let fields = [("name", "linear"), ("url", LINEAR), ("key", KEY)];
    let ack = submit(rig, "env-2", user, &view, &fields);
    assert_eq!(ack["response_action"], "update", "{ack}");
}

#[test]
fn mcp_add_opens_a_private_modal_with_name_url_and_key() {
    let (rig, _controller) = rig();
    let view = open_modal(&rig, "env-1", USER);
    assert_eq!(view["callback_id"], "moor_mcp");
    let blocks = view["blocks"].as_array().unwrap();
    let inputs: Vec<&str> = blocks
        .iter()
        .filter(|b| b["type"] == "input")
        .map(|b| b["block_id"].as_str().unwrap())
        .collect();
    assert_eq!(inputs, ["name", "url", "key"]);
    let shown = view.to_string();
    assert!(shown.contains("Bearer token or API key. Visible while you type, only to you."));
    assert!(shown.contains(
        "Example: Linear read-only. URL https://mcp.linear.app/mcp/readonly with a Linear \
         personal API key that has only Read permission."
    ));
}

#[test]
fn a_submission_stores_the_server_and_key_for_that_slack_user_only() {
    let (rig, _controller) = rig();
    add_linear(&rig, USER);
    let front = rig.front_socket();
    assert_eq!(
        servers(&front, USER),
        [("linear".to_owned(), LINEAR.to_owned(), true)]
    );
    assert_eq!(servers(&front, OTHER), []);
}

#[test]
fn the_key_is_in_no_slack_post_ack_or_log_line() {
    let (rig, _controller) = rig();
    add_linear(&rig, USER);
    rig.slack.send_slash("env-3", CHANNEL, USER, "mcp list");
    rig.slack.wait_responses(1);
    let seen: Vec<String> = rig
        .slack
        .requests()
        .iter()
        .map(|r| format!("{} {} {}", r.path, r.body, r.reply))
        .collect();
    assert!(
        !seen.iter().any(|s| s.contains(KEY)),
        "the key reached Slack"
    );
    let ack = rig.slack.wait_ack_payload("env-2");
    assert!(!ack.to_string().contains(KEY), "the key reached the ack");
    assert!(!rig.stderr().contains(KEY), "the key reached the log");
}

#[test]
fn list_shows_names_and_urls_privately_and_never_the_key() {
    let (rig, _controller) = rig();
    add_linear(&rig, USER);
    rig.slack.send_slash("env-3", CHANNEL, USER, "mcp list");
    let answers = rig.slack.wait_responses(1);
    let (id, text) = &answers[0];
    assert_eq!(id, "env-3");
    assert!(text.contains("linear"), "{text}");
    assert!(text.contains(LINEAR), "{text}");
    assert!(text.contains("key stored"), "{text}");
    assert!(!text.contains(KEY), "{text}");
    rig.slack.send_slash("env-4", CHANNEL, OTHER, "mcp list");
    let answers = rig.slack.wait_responses(2);
    assert!(!answers[1].1.contains("linear"), "{}", answers[1].1);
}

#[test]
fn remove_drops_the_server_and_confirms_privately() {
    let (rig, _controller) = rig();
    add_linear(&rig, USER);
    rig.slack
        .send_slash("env-3", CHANNEL, USER, "mcp remove linear");
    let answers = rig.slack.wait_responses(1);
    assert!(
        answers[0].1.contains("Removed `linear`"),
        "{}",
        answers[0].1
    );
    assert_eq!(servers(&rig.front_socket(), USER), []);
}

#[test]
fn a_controller_refusal_shows_on_the_modal_field() {
    let (rig, _controller) = rig();
    let view = open_modal(&rig, "env-1", USER);
    let bad_url = [
        ("name", "linear"),
        ("url", "http://example.com/mcp"),
        ("key", KEY),
    ];
    let ack = submit(&rig, "env-2", USER, &view, &bad_url);
    assert_eq!(ack["response_action"], "errors", "{ack}");
    assert!(
        ack["errors"]["url"].as_str().unwrap().contains("https"),
        "{ack}"
    );
    let bad_name = [("name", "Not A Name"), ("url", LINEAR), ("key", KEY)];
    let ack = submit(&rig, "env-3", USER, &view, &bad_name);
    assert_eq!(ack["response_action"], "errors", "{ack}");
    assert!(ack["errors"]["name"].is_string(), "{ack}");
    assert_eq!(servers(&rig.front_socket(), USER), []);
}
