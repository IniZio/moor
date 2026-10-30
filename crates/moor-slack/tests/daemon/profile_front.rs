//! `/moor profile` against a real controller: each Slack user's profile is their own, and a
//! refusal reaches the person as worded.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::Path;

use moor_client::Client;
use moor_core::request::{AsReq, EnrollReq, Request};
use moor_core::response::{ExitClass, Response};
use moor_core::user::Identity;

use super::support::slack_fake::TEAM;
use super::support::{Rig, CHANNEL, USER};

/// A second Slack user.
const OTHER: &str = "U0SECOND";

const REPO: &str = "https://github.com/acme/dots";

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

/// The profile the controller holds for `user`.
fn stored(front: &Path, user: &str) -> Option<String> {
    let shown = ask_as(front, user, Request::ProfileShow(AsReq { identity: None }));
    shown.value.unwrap()["source"].as_str().map(str::to_owned)
}

fn rig() -> (Rig, super::support::RealController) {
    let mut rig = Rig::new();
    let controller = rig.real_controller();
    let front = rig.front_socket();
    for user in [USER, OTHER] {
        let enroll = Request::Enroll(EnrollReq {
            identity: None,
            email: None,
        });
        ask_as(&front, user, enroll);
    }
    rig.spawn_daemon_on_front();
    rig.slack.wait_connected();
    (rig, controller)
}

#[test]
fn set_show_and_remove_round_trip_as_that_user_only() {
    let (rig, _controller) = rig();
    let front = rig.front_socket();
    rig.slack.send_slash("e1", CHANNEL, USER, "profile");
    let answers = rig.slack.wait_responses(1);
    assert!(
        answers[0].1.contains("You have no agent profile"),
        "{}",
        answers[0].1
    );

    rig.slack
        .send_slash("e2", CHANNEL, USER, &format!("profile set {REPO}#dev"));
    let answers = rig.slack.wait_responses(2);
    assert!(
        answers[1].1.contains(&format!("{REPO}#dev")),
        "{}",
        answers[1].1
    );
    assert_eq!(
        stored(&front, USER).as_deref(),
        Some(&*format!("{REPO}#dev"))
    );
    assert_eq!(stored(&front, OTHER), None);

    rig.slack.send_slash("e3", CHANNEL, USER, "profile");
    let answers = rig.slack.wait_responses(3);
    assert!(
        answers[2].1.contains(&format!("{REPO}#dev")),
        "{}",
        answers[2].1
    );
    rig.slack.send_slash("e4", CHANNEL, OTHER, "profile");
    let answers = rig.slack.wait_responses(4);
    assert!(
        answers[3].1.contains("You have no agent profile"),
        "{}",
        answers[3].1
    );

    rig.slack.send_slash("e5", CHANNEL, USER, "profile remove");
    let answers = rig.slack.wait_responses(5);
    assert!(answers[4].1.contains("Removed"), "{}", answers[4].1);
    assert_eq!(stored(&front, USER), None);
    rig.slack.send_slash("e6", CHANNEL, USER, "profile remove");
    let answers = rig.slack.wait_responses(6);
    assert!(
        answers[5].1.contains("had no agent profile"),
        "{}",
        answers[5].1
    );
}

#[test]
fn a_url_the_controller_cannot_take_is_refused_in_its_words() {
    let (rig, _controller) = rig();
    rig.slack.send_slash(
        "e1",
        CHANNEL,
        USER,
        "profile set https://gitlab.com/acme/dots",
    );
    let ack = rig.slack.wait_ack_payload("e1");
    let text = ack["text"].as_str().unwrap();
    assert!(
        text.contains(
            "a profile must be https://github.com/OWNER/NAME, optionally followed by #REF"
        ),
        "{text}"
    );
    assert_eq!(stored(&rig.front_socket(), USER), None);
}
