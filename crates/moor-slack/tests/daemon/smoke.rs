//! Smoke tests against a real `moor controller serve` with no repos, reached only through
//! its socket. The fake `claude` calls `start_box` in every turn.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::Path;

use moor_client::Client;
use moor_core::name::{BoxName, SessionId};
use moor_core::request::{
    AsReq, EnrollReq, Request, SecretSetReq, SecretValue, SessionsReq, ShowReq,
    UpdateHostSessionReq,
};
use moor_core::response::{ExitClass, Response};
use moor_core::user::{Identity, SecretName};
use serde_json::json;
use std::time::Duration;

use super::support::slack_fake::TEAM;
use super::support::{Rig, BOX_NAME, MCP, TS, USER};

/// The refusal the controller gives an enrolled person with only a Claude secret, before it
/// reads any repo.
const UNKNOWN: &str =
    "A token you have not linked is needed. Link it with `/moor login`, then try again.";

/// A daemon on a real controller whose channel's default repo is `o/nosuch`.
fn rig_on_a_real_controller() -> (Rig, super::support::RealController) {
    let mut rig = Rig::new();
    let bin = rig.use_fake_claude(None);
    Rig::tool_calls(
        &bin,
        None,
        &[("start_box", json!({"prompt": "fix the thing"}))],
    );
    let controller = rig.real_controller();
    enroll(&rig.front_socket(), USER);
    log_in_claude(&rig.front_socket(), USER, &bin);
    rig.spawn_daemon_on_front();
    rig.slack.wait_connected();
    rig.set_default_repo("o/nosuch");
    (rig, controller)
}

#[test]
fn real_controller_refusal_is_posted_in_the_thread() {
    let (rig, _controller) = rig_on_a_real_controller();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_post(&format!("Could not start Box `{BOX_NAME}`: {UNKNOWN}"));
}

/// The controller stops after the first thread's start and a new one takes its socket; the
/// second thread opens a host Session on it and gets its refusal.
#[test]
fn real_controller_restart_is_survived() {
    let (rig, first) = rig_on_a_real_controller();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_post(&format!("Could not start Box `{BOX_NAME}`: {UNKNOWN}"));
    drop(first);
    let _second = rig.real_controller();
    let ts = "1730000009.000300";
    rig.mention(2, USER, "<@UBOT> fix the thing", ts, None);
    rig.wait_post(&format!(
        "Could not start Box `slack-1730000009-000300-1`: {UNKNOWN}"
    ));
}

/// The identity the controller knows Slack user `user` of the fake team by.
fn slack_identity(user: &str) -> Identity {
    Identity::parse(&format!("slack:{TEAM}/{user}")).unwrap()
}

/// Enrolls `user` through the front socket, asserting the identity in the request.
fn enroll(front: &Path, user: &str) {
    let enroll = Request::Enroll(EnrollReq {
        identity: Some(slack_identity(user)),
        email: None,
    });
    let reply = Client::new(front).request(enroll, |_| {}).unwrap();
    assert_eq!(reply.class, ExitClass::Ok, "{reply:?}");
}

/// Stores a Claude secret for `user` through the front socket, as `/moor login claude` does,
/// and has the fake `claude` in `bin` answer whatever lease the controller grants as [`MCP`].
fn log_in_claude(front: &Path, user: &str, bin: &Path) {
    let set = Request::SecretSet(SecretSetReq {
        identity: None,
        name: SecretName::parse("claude").unwrap(),
        value: SecretValue::new(b"sk-ant-oat01-smoke".to_vec()),
    });
    let reply = ask_as(front, user, set);
    assert_eq!(reply.class, ExitClass::Ok, "{reply:?}");
    std::fs::write(bin.join("mode"), MCP).unwrap();
}

/// The reply to `request` from the controller socket `socket`, sent as Slack user `user`.
fn ask_as(socket: &Path, user: &str, request: Request) -> Response {
    let client = Client::new(socket).acting_as(slack_identity(user));
    client.request(request, |_| {}).unwrap()
}

/// The ids of the host Sessions `user` sees.
fn sessions(front: &Path, user: &str) -> Vec<i64> {
    let reply = ask_as(front, user, Request::Sessions(SessionsReq { parent: None }));
    assert_eq!(reply.class, ExitClass::Ok, "{reply:?}");
    let listed = reply.value.unwrap()["sessions"].clone();
    let ids = listed.as_array().unwrap().iter();
    ids.filter_map(|s| s["session"].as_i64()).collect()
}

fn show(name: &str) -> Request {
    Request::Show(ShowReq {
        target: Some(BoxName::parse_allow_bare(name).unwrap().into()),
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    })
}

/// Asserts `reply` is a refusal with `message`.
fn assert_refused(reply: &Response, message: &str) {
    assert_eq!(reply.class, ExitClass::Refused, "{reply:?}");
    let error = reply.error.as_ref().unwrap();
    assert!(error.message.contains(message), "{reply:?}");
}

/// The user who stays out of the first user's Boxes and Sessions.
const OTHER: &str = "U0SECOND";

#[test]
fn on_the_front_socket_the_front_acts_for_one_slack_user_and_no_other_sees_it() {
    let (rig, _controller) = rig_on_a_real_controller();
    let front = rig.front_socket();
    for user in [USER, OTHER] {
        enroll(&front, user);
    }
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_post(&format!("Could not start Box `{BOX_NAME}`: {UNKNOWN}"));
    assert_eq!(sessions(&front, USER).len(), 1, "the owner's host Session");
    assert_eq!(sessions(&front, OTHER), Vec::<i64>::new());
    let theirs = sessions(&front, USER)[0];
    let touch = Request::UpdateHostSession(UpdateHostSessionReq {
        session: SessionId::new(theirs),
        agent: None,
        agent_session_id: None,
    });
    assert_refused(&ask_as(&front, OTHER, touch), "not your Box");
}

#[test]
fn on_the_operator_socket_the_same_request_path_is_refused() {
    let mut rig = Rig::new();
    let bin = rig.use_fake_claude(None);
    Rig::tool_calls(
        &bin,
        None,
        &[("start_box", json!({"prompt": "fix the thing"}))],
    );
    let _controller = rig.real_controller();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.set_default_repo("o/nosuch");
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.slack.wait_until("the refusal", |posts, _| {
        posts
            .iter()
            .any(|p| p.text.contains("speaks only for local:"))
    });
    let direct = ask_as(&rig.socket(), USER, show(BOX_NAME));
    assert_refused(&direct, "speaks only for local:");
}

/// The names of the secrets in the vault of Slack user `user`, read through the front socket.
fn secret_names(front: &Path, user: &str) -> Vec<String> {
    let reply = ask_as(front, user, Request::SecretNames(AsReq { identity: None }));
    assert_eq!(reply.class, ExitClass::Ok, "{reply:?}");
    let names = reply.value.unwrap()["names"].clone();
    let names = names.as_array().unwrap().iter();
    names
        .filter_map(|n| n.as_str().map(str::to_owned))
        .collect()
}

#[test]
fn login_github_through_the_front_fills_that_users_vault_in_a_real_controller_and_no_one_elses() {
    let mut rig = Rig::new();
    let _controller = rig.real_controller();
    rig.spawn_daemon_on_front();
    rig.slack.wait_connected();
    let front = rig.front_socket();
    rig.slack
        .send_slash("env-1", super::support::CHANNEL, USER, "login github");
    let opened = rig
        .slack
        .wait_views("views.open", 1, Duration::from_secs(10));
    let token = "gho_fake_real_vault_5c20e1";
    rig.slack
        .send_submission("env-2", USER, &opened[0]["view"], token);
    let updated = rig
        .slack
        .wait_views("views.update", 1, Duration::from_secs(20));
    let text = updated[0]["view"]["blocks"][0]["text"]["text"]
        .as_str()
        .unwrap();
    assert_eq!(text, "Linked.", "{}", rig.stderr());
    assert_eq!(secret_names(&front, USER), ["github"]);
    enroll(&front, OTHER);
    assert_eq!(secret_names(&front, OTHER), Vec::<String>::new());
    assert!(!rig.stderr().contains(token), "the token reached the log");
}
