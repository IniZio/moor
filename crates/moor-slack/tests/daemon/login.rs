//! `/moor login` and `/moor logout`, and how a Slack user's first request enrolls them, against
//! the scripted controller and the fake Slack.

#![allow(clippy::indexing_slicing, reason = "test code")]

use moor_core::controller::PROTOCOL;
use moor_core::request::Request;

use super::support::controller::{failed, FakeController};
use super::support::slack_fake::TEAM;
use super::support::{Rig, CHANNEL, QUIET, TS, USER};

/// The profile a first request sets, which enrolls the user.
const PROFILE: &str = "https://github.com/acme/dots";

/// A rig whose daemon runs against the scripted controller.
fn start() -> (Rig, FakeController) {
    let mut rig = Rig::new();
    let controller = rig.serve(PROTOCOL);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    (rig, controller)
}

/// The requests the controller got as `(identity, command)`, in order, without the plain ones
/// the front sends for its own turns.
fn asked(controller: &FakeController) -> Vec<(String, &'static str)> {
    controller.seen(|seen| {
        let who = seen.acting.iter().map(|w| w.clone().unwrap_or_default());
        who.zip(&seen.requests)
            .filter_map(|(who, request)| match request {
                Request::Enroll(_) => Some((who, "enroll")),
                Request::SecretSet(_) => Some((who, "secret-set")),
                Request::SecretDelete(_) => Some((who, "secret-delete")),
                _ => None,
            })
            .collect()
    })
}

fn identity(user: &str) -> String {
    format!("slack:{TEAM}/{user}")
}

#[test]
fn login_answers_with_the_account_page_link_and_opens_no_modal() {
    let (rig, controller) = start();
    for (id, text) in [("env-1", "login"), ("env-2", "login github")] {
        rig.slack.send_slash(id, CHANNEL, USER, text);
        let ack = rig.slack.wait_ack_payload(id);
        let said = ack["text"].as_str().unwrap();
        assert!(said.contains("https://moor.example.com/"), "{said}");
    }
    std::thread::sleep(QUIET);
    assert!(rig.slack.views("views.open").is_empty(), "no modal opens");
    assert!(asked(&controller).is_empty(), "{:?}", asked(&controller));
}

#[test]
fn logout_deletes_the_named_secret_as_that_user_and_the_answer_says_so() {
    let (rig, controller) = start();
    rig.slack
        .send_slash("env-3", CHANNEL, USER, "logout github");
    let ack = rig.slack.wait_ack_payload("env-3");
    assert_eq!(ack["text"], "Unlinking your GitHub token…");
    let answers = rig.slack.wait_responses(1);
    assert_eq!(
        answers,
        [("env-3".to_owned(), "Unlinked your GitHub token.".to_owned())]
    );
    assert_eq!(asked(&controller), [(identity(USER), "secret-delete")]);
    let named = controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::SecretDelete(d) => Some(d.name.as_str().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(named, ["github"]);
    rig.slack.send_slash("env-4", CHANNEL, USER, "logout");
    rig.slack.wait_responses(2);
    let named = controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::SecretDelete(d) => Some(d.name.as_str().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(named, ["github", "claude"]);
}

#[test]
fn a_logout_the_controller_refuses_says_why_to_the_user() {
    let (rig, controller) = start();
    let refusal = failed("secret-delete", "log in first");
    controller.script(
        "secret-delete",
        super::support::controller::Scripted::Reply(refusal),
    );
    rig.slack
        .send_slash("env-3", CHANNEL, USER, "logout claude");
    let answers = rig.slack.wait_responses(1);
    assert_eq!(
        answers[0].1,
        "Could not unlink your Claude token: log in first"
    );
}

#[test]
fn a_user_nobody_listed_mentions_the_bot_in_an_invited_channel_and_a_thread_opens() {
    let (rig, _controller) = start();
    rig.mention(1, "U0NEWBIE", "<@UBOT> hello", TS, None);
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let owner = |db: &rusqlite::Connection| -> Option<String> {
        db.query_row("SELECT owner FROM slack_threads", [], |r| r.get(0))
            .ok()
    };
    let mut found = None;
    for _ in 0..500 {
        found = owner(&db);
        if found.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(found.as_deref(), Some("U0NEWBIE"));
    assert!(!rig
        .slack
        .posts()
        .iter()
        .any(|p| p.text.contains("not allowed")));
}

#[test]
fn a_mention_in_a_channel_nobody_configured_opens_a_thread() {
    let (rig, _controller) = start();
    rig.slack
        .send_mention(&super::support::slack_fake::Mention {
            event_id: "Ev1",
            envelope_id: "env-1",
            channel: "C0ELSEWHERE",
            user: USER,
            text: "<@UBOT> hello",
            ts: TS,
            thread_ts: None,
        });
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let mut found = None;
    for _ in 0..500 {
        found = db
            .query_row("SELECT channel FROM slack_threads", [], |r| {
                r.get::<_, String>(0)
            })
            .ok();
        if found.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(found.as_deref(), Some("C0ELSEWHERE"));
}

/// The email each `enroll` the controller got carried, in order.
fn enroll_emails(controller: &FakeController) -> Vec<Option<String>> {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|request| match request {
                Request::Enroll(enroll) => {
                    Some(enroll.email.as_ref().map(|e| e.as_str().to_owned()))
                }
                _ => None,
            })
            .collect()
    })
}

#[test]
fn an_enroll_carries_the_slack_profile_email_and_the_email_is_read_once() {
    let (rig, controller) = start();
    rig.slack.set_email(USER, "Alice@Example.com");
    rig.slack
        .send_slash("env-1", CHANNEL, USER, &format!("profile set {PROFILE}"));
    rig.slack.wait_responses(1);
    assert_eq!(
        enroll_emails(&controller),
        [Some("alice@example.com".to_owned())]
    );
    assert_eq!(rig.slack.users_info_calls(), 1);
    rig.slack.send_slash(
        "env-2",
        CHANNEL,
        USER,
        &format!("profile set {PROFILE}#dev"),
    );
    rig.slack.wait_responses(2);
    assert_eq!(
        enroll_emails(&controller),
        [
            Some("alice@example.com".to_owned()),
            Some("alice@example.com".to_owned())
        ]
    );
    assert_eq!(
        rig.slack.users_info_calls(),
        1,
        "the second enroll used the remembered email"
    );
}

#[test]
fn a_bot_scope_that_is_missing_enrolls_without_an_email_and_logs_the_scope_once() {
    let (rig, controller) = start();
    rig.slack.refuse_users_info("missing_scope");
    rig.slack
        .send_slash("env-1", CHANNEL, USER, &format!("profile set {PROFILE}"));
    rig.slack.wait_responses(1);
    assert_eq!(enroll_emails(&controller), [None]);
    let stderr = rig.stderr();
    assert_eq!(stderr.matches("users:read.email").count(), 1, "{stderr}");
}

#[test]
fn a_profile_without_an_email_enrolls_without_one() {
    let (rig, controller) = start();
    rig.slack
        .send_slash("env-1", CHANNEL, USER, &format!("profile set {PROFILE}"));
    rig.slack.wait_responses(1);
    assert_eq!(enroll_emails(&controller), [None]);
    assert_eq!(rig.slack.users_info_calls(), 1);
}
