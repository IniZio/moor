//! `/moor machine add`, `list` and `remove` against a real controller: each Slack user's machines
//! are their own, and a refusal reaches the person as the controller worded it.

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

/// The machines `user` has, as `(name, ssh, labels)`, asked of the controller itself.
fn machines(front: &Path, user: &str) -> Vec<(String, String, Vec<String>)> {
    let listed = ask_as(front, user, Request::MachineList(AsReq { identity: None }));
    let machines = listed.value.unwrap()["machines"].clone();
    machines
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let text = |k: &str| m[k].as_str().unwrap().to_owned();
            let labels = m["labels"].as_array().unwrap();
            let labels = labels.iter().map(|l| l.as_str().unwrap().to_owned());
            (text("name"), text("ssh"), labels.collect())
        })
        .collect()
}

fn rig() -> (Rig, super::support::RealController) {
    let mut rig = Rig::new();
    let controller = rig.real_controller();
    let front = rig.front_socket();
    for user in [USER, OTHER] {
        ask_as(&front, user, Request::Enroll(AsReq { identity: None }));
    }
    rig.spawn_daemon_on_front();
    rig.slack.wait_connected();
    (rig, controller)
}

/// Runs `/moor machine add` as `user`; returns the modal it opened.
fn open_modal(rig: &Rig, envelope: &str, user: &str) -> Value {
    rig.slack.send_slash(envelope, CHANNEL, user, "machine add");
    let opened = rig.slack.wait_views("views.open", 1, WAIT);
    assert_eq!(opened[0]["trigger_id"], format!("trigger-{envelope}"));
    opened[0]["view"].clone()
}

/// Runs `/moor machine add` as `user` for the `n`th time; returns the modal it opened.
fn open_modal_nth(rig: &Rig, envelope: &str, user: &str, n: usize) -> Value {
    rig.slack.send_slash(envelope, CHANNEL, user, "machine add");
    let opened = rig.slack.wait_views("views.open", n, WAIT);
    opened[n - 1]["view"].clone()
}

/// What the modal shows for a capacity it cannot use.
const BAD_CAPACITY: &str = "Capacity must be a whole number from 1 to 65535. Nothing was stored.";

/// Submits the modal as `user` with these fields; returns the ack's payload.
fn submit(rig: &Rig, envelope: &str, user: &str, view: &Value, fields: &[(&str, &str)]) -> Value {
    rig.slack.send_fields(envelope, user, view, fields);
    rig.slack.wait_ack_payload(envelope)
}

/// Adds `minion` with a label as `user`.
fn add_minion(rig: &Rig, user: &str) {
    let view = open_modal(rig, "env-1", user);
    let fields = [
        ("name", "minion"),
        ("ssh", "moor@minion"),
        ("labels", "gpu, big"),
    ];
    let ack = submit(rig, "env-2", user, &view, &fields);
    assert_eq!(ack["response_action"], "update", "{ack}");
}

#[test]
fn machine_add_opens_a_modal_with_name_ssh_target_and_labels() {
    let (rig, _controller) = rig();
    let view = open_modal(&rig, "env-1", USER);
    assert_eq!(view["callback_id"], "moor_machine");
    let inputs: Vec<(&str, bool)> = view["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "input")
        .map(|b| {
            let optional = b["optional"].as_bool().unwrap();
            (b["block_id"].as_str().unwrap(), optional)
        })
        .collect();
    let want = [
        ("name", false),
        ("ssh", false),
        ("labels", true),
        ("capacity", true),
    ];
    assert_eq!(inputs, want);
}

#[test]
fn a_submission_sends_machine_add_with_the_three_fields_as_that_user() {
    let (rig, _controller) = rig();
    add_minion(&rig, USER);
    let front = rig.front_socket();
    let labels = vec!["big".to_owned(), "gpu".to_owned()];
    assert_eq!(
        machines(&front, USER),
        [("minion".to_owned(), "moor@minion".to_owned(), labels)]
    );
    assert_eq!(machines(&front, OTHER), []);
}

/// The capacity the controller stores for `user`'s machine `name`.
fn capacity(front: &Path, user: &str, name: &str) -> Option<u64> {
    let listed = ask_as(front, user, Request::MachineList(AsReq { identity: None }));
    let machines = listed.value.unwrap()["machines"].clone();
    let found = machines
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .cloned();
    found.map(|m| m["capacity"].as_u64().unwrap_or(1))
}

#[test]
fn a_submission_with_a_capacity_stores_it() {
    let (rig, _controller) = rig();
    let view = open_modal(&rig, "env-1", USER);
    let fields = [
        ("name", "minion"),
        ("ssh", "moor@minion"),
        ("capacity", " 2 "),
    ];
    let ack = submit(&rig, "env-2", USER, &view, &fields);
    assert_eq!(ack["response_action"], "update", "{ack}");
    let shown = &ack["view"]["blocks"][0]["text"]["text"];
    assert_eq!(
        capacity(&rig.front_socket(), USER, "minion"),
        Some(2),
        "{shown}"
    );
}

#[test]
fn a_capacity_that_is_not_a_whole_number_from_one_stores_nothing() {
    let (rig, _controller) = rig();
    for (n, bad) in ["0", "two", "-1", "70000"].iter().enumerate() {
        let view = open_modal_nth(&rig, &format!("env-o{n}"), USER, n + 1);
        let fields = [
            ("name", "minion"),
            ("ssh", "moor@minion"),
            ("capacity", *bad),
        ];
        let ack = submit(&rig, &format!("env-s{n}"), USER, &view, &fields);
        let shown = ack["view"]["blocks"][0]["text"]["text"].as_str().unwrap();
        assert_eq!(shown, BAD_CAPACITY, "{bad}");
    }
    assert_eq!(capacity(&rig.front_socket(), USER, "minion"), None);
}

#[test]
fn a_controller_refusal_reaches_the_person_word_for_word() {
    let (rig, _controller) = rig();
    let view = open_modal(&rig, "env-1", USER);
    let fields = [("name", "Not A Name"), ("ssh", "moor@minion")];
    let ack = submit(&rig, "env-2", USER, &view, &fields);
    assert_eq!(ack["response_action"], "update", "{ack}");
    let shown = ack["view"]["blocks"][0]["text"]["text"].as_str().unwrap();
    assert!(
        shown.contains("machine name `Not A Name` must match [a-z][a-z0-9_-]* in at most 64 bytes"),
        "{shown}"
    );
    assert_eq!(machines(&rig.front_socket(), USER), []);
}

#[test]
fn list_shows_the_persons_machines_and_says_when_there_are_none() {
    let (rig, _controller) = rig();
    rig.slack.send_slash("env-0", CHANNEL, USER, "machine list");
    let none = rig.slack.wait_responses(1);
    assert!(none[0].1.contains("You have no machines"), "{}", none[0].1);
    add_minion(&rig, USER);
    rig.slack.send_slash("env-3", CHANNEL, USER, "machine list");
    let answers = rig.slack.wait_responses(2);
    let text = &answers[1].1;
    for part in ["minion", "moor@minion", "big", "gpu"] {
        assert!(text.contains(part), "{text}");
    }
    rig.slack
        .send_slash("env-4", CHANNEL, OTHER, "machine list");
    let answers = rig.slack.wait_responses(3);
    assert!(!answers[2].1.contains("minion"), "{}", answers[2].1);
}

#[test]
fn remove_drops_a_present_machine_and_says_when_there_was_none() {
    let (rig, _controller) = rig();
    add_minion(&rig, USER);
    rig.slack
        .send_slash("env-3", CHANNEL, USER, "machine remove minion");
    let answers = rig.slack.wait_responses(1);
    assert!(
        answers[0].1.contains("Removed `minion`"),
        "{}",
        answers[0].1
    );
    assert_eq!(machines(&rig.front_socket(), USER), []);
    rig.slack
        .send_slash("env-4", CHANNEL, USER, "machine remove minion");
    let answers = rig.slack.wait_responses(2);
    assert!(
        answers[1].1.contains("You have no machine named `minion`"),
        "{}",
        answers[1].1
    );
}

#[test]
fn any_other_machine_command_gets_the_usage_line() {
    let (rig, _controller) = rig();
    rig.slack
        .send_slash("env-1", CHANNEL, USER, "machine frobnicate");
    let ack = rig.slack.wait_ack_payload("env-1");
    let text = ack["text"].as_str().unwrap();
    assert!(text.starts_with("Usage: `/moor machine add`"), "{text}");
    assert!(text.contains("`/moor machine remove <name>`"), "{text}");
}
