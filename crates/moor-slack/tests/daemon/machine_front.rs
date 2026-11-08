//! `/moor machine add`, `list` and `remove` against a real controller: each Slack user's machines
//! are their own, and a refusal reaches the person as the controller worded it.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::Path;

use moor_client::Client;
use moor_core::request::{AsReq, EnrollReq, Request};
use moor_core::response::{ExitClass, Response};
use moor_core::user::Identity;
use serde_json::json;

use super::support::slack_fake::TEAM;
use super::support::{Rig, CHANNEL, USER};

/// A second Slack user.
const OTHER: &str = "U0SECOND";

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

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

/// The names of the machines `user` has, asked of the controller itself.
fn machines(front: &Path, user: &str) -> Vec<String> {
    let listed = ask_as(front, user, Request::MachineList(AsReq { identity: None }));
    let machines = listed.value.unwrap()["machines"].clone();
    let machines = machines.as_array().unwrap().iter();
    machines
        .map(|m| m["name"].as_str().unwrap().to_owned())
        .collect()
}

fn rig() -> (Rig, super::support::RealController) {
    let mut rig = Rig::new();
    let controller = rig.real_controller();
    let front = rig.front_socket();
    for user in [USER, OTHER] {
        ask_as(
            &front,
            user,
            Request::Enroll(EnrollReq {
                identity: None,
                email: None,
            }),
        );
    }
    rig.spawn_daemon_on_front();
    rig.slack.wait_connected();
    (rig, controller)
}

/// Enrolls `hv`, a Box pool machine, for `user` and brings its agent link up.
fn add_hv(rig: &Rig, user: &str) {
    let enroll = json!({"cmd": "machine-enroll", "name": "hv", "pubkey": KEY,
        "labels": [], "pool": "cloudhypervisor"});
    ask_as(
        &rig.front_socket(),
        user,
        serde_json::from_value(enroll).unwrap(),
    );
}

/// Enrolls `minion` with two labels for `user`, as its agent would.
fn add_minion(rig: &Rig, user: &str) {
    let enroll = json!({"cmd": "machine-enroll", "name": "minion", "pubkey": KEY,
        "labels": ["gpu", "big"]});
    ask_as(
        &rig.front_socket(),
        user,
        serde_json::from_value(enroll).unwrap(),
    );
}

#[test]
fn machine_add_gives_the_install_and_up_commands_and_stores_nothing() {
    let (rig, _controller) = rig();
    rig.slack.send_slash("env-1", CHANNEL, USER, "machine add");
    let ack = rig.slack.wait_ack_payload("env-1");
    let text = ack["text"].as_str().unwrap();
    assert!(
        text.contains("install.sh | sh -s moor-machine-agent"),
        "{text}"
    );
    assert!(
        text.contains("moor-machine-agent up --server <url>"),
        "{text}"
    );
    assert!(!text.contains("enroll"), "{text}");
    assert!(machines(&rig.front_socket(), USER).is_empty());
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
    for part in ["minion", "big", "gpu"] {
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
    assert!(machines(&rig.front_socket(), USER).is_empty());
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
    assert!(text.starts_with("Usage: `/moor machine list`"), "{text}");
    assert!(text.contains("`/moor machine remove <name>`"), "{text}");
}

#[test]
fn list_says_a_machine_hosts_no_boxes_until_it_has_a_runtime_and_a_link() {
    let (rig, _controller) = rig();
    add_minion(&rig, USER);
    add_hv(&rig, USER);
    rig.slack.send_slash("env-5", CHANNEL, USER, "machine list");
    let answers = rig.slack.wait_responses(1);
    let text = &answers[0].1;
    assert!(
        text.contains("minion") && text.contains("hosts no Boxes"),
        "{text}"
    );
    assert!(text.contains("hv") && text.contains("offline"), "{text}");
    assert!(text.contains("A start with no node named fails"), "{text}");
}
