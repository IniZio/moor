//! Starting a Box from a mention, against the scripted controller.

#![allow(clippy::indexing_slicing, reason = "test code")]

use moor_core::controller::PROTOCOL;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::support::controller::{FakeController, Scripted};
use super::support::slack_fake::{Mention, APP_TOKEN, BOT_TOKEN};
use super::support::{Rig, BOX_NAME, CHANNEL, QUIET, TS, USER};

/// A `start` or `show` reply that names Box 7 called `name`.
fn names_box(cmd: &str, name: &str) -> Scripted {
    let value = json!({"box": {"id": 7, "name": name, "branch": format!("moor/{name}")}});
    Scripted::Reply(Response::ok(cmd, Some(value)))
}

/// The name, repo and goal of every `start` the controller received.
fn starts(controller: &FakeController) -> Vec<(String, Option<String>, Option<String>)> {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::Start(s) => {
                    Some((s.name.as_str().to_owned(), s.repo.clone(), s.goal.clone()))
                }
                _ => None,
            })
            .collect()
    })
}

#[test]
fn allowed_mention_starts_a_box_and_replies_in_the_thread() {
    let (rig, controller) = Rig::start();
    controller.script("start", names_box("start", BOX_NAME));
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    assert_eq!(rig.slack.wait_acks(1), ["env-1"]);
    let posts = rig.slack.wait_posts(1);
    assert_eq!(posts[0].channel, CHANNEL);
    assert_eq!(posts[0].thread_ts, TS);
    assert!(posts[0].text.contains(BOX_NAME), "{}", posts[0].text);
    let want = (
        BOX_NAME.to_owned(),
        Some("demo".to_owned()),
        Some("fix the thing".to_owned()),
    );
    assert_eq!(starts(&controller), [want]);
    let requests = rig.slack.requests();
    assert!(requests
        .iter()
        .filter(|r| r.path == "/apps.connections.open")
        .all(|r| r.auth == format!("Bearer {APP_TOKEN}")));
    assert!(requests
        .iter()
        .filter(|r| r.path == "/chat.postMessage")
        .all(|r| r.auth == format!("Bearer {BOT_TOKEN}")));
}

#[test]
fn redelivered_event_is_handled_once() {
    let (rig, controller) = Rig::start();
    controller.script("start", names_box("start", BOX_NAME));
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    rig.slack.send_mention(&Mention {
        event_id: "Ev1",
        envelope_id: "env-1-retry",
        channel: CHANNEL,
        user: USER,
        text: "<@UBOT> start demo fix the thing",
        ts: TS,
        thread_ts: None,
    });
    assert_eq!(rig.slack.wait_acks(2), ["env-1", "env-1-retry"]);
    rig.slack.wait_posts(1);
    std::thread::sleep(QUIET);
    assert_eq!(rig.slack.posts().len(), 1, "{:?}", rig.slack.posts());
    assert_eq!(starts(&controller).len(), 1);
}

#[test]
fn second_mention_in_a_thread_starts_nothing_new() {
    let (rig, controller) = Rig::start();
    controller.script("start", names_box("start", BOX_NAME));
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    rig.slack.wait_posts(1);
    rig.mention(
        2,
        USER,
        "<@UBOT> start demo and another thing",
        "1730000005.000200",
        Some(TS),
    );
    let posts = rig.slack.wait_posts(2);
    assert_eq!(posts[1].thread_ts, TS);
    assert!(posts[1].text.contains(BOX_NAME), "{}", posts[1].text);
    assert!(!posts[1].text.contains("000200"), "{}", posts[1].text);
    assert_eq!(starts(&controller).len(), 1);
}

#[test]
fn user_off_the_allow_list_is_refused_and_nothing_starts() {
    let (rig, controller) = Rig::start();
    rig.mention(1, "U0OTHER", "<@UBOT> start demo fix the thing", TS, None);
    let posts = rig.slack.wait_posts(1);
    assert_eq!(posts[0].thread_ts, TS);
    assert!(posts[0].text.contains("<@U0OTHER>"), "{}", posts[0].text);
    std::thread::sleep(QUIET);
    assert_eq!(rig.slack.posts().len(), 1);
    assert!(controller.seen(|seen| seen.requests.is_empty()));
}

#[test]
fn tokens_never_reach_stderr() {
    let (rig, controller) = Rig::start();
    controller.script("start", Scripted::Hangup);
    controller.script("show", names_box("show", BOX_NAME));
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    rig.slack.wait_posts(1);
    let stderr = rig.stderr();
    assert!(
        stderr.contains(BOX_NAME),
        "the lost start is logged: {stderr}"
    );
    assert!(!stderr.contains(APP_TOKEN), "{stderr}");
    assert!(!stderr.contains(BOT_TOKEN), "{stderr}");
}

#[test]
fn start_lost_mid_request_is_found_with_show() {
    let (rig, controller) = Rig::start();
    controller.script("start", Scripted::Hangup);
    controller.script("show", names_box("show", BOX_NAME));
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    let posts = rig.slack.wait_posts(1);
    assert_eq!(posts[0].text, format!("Started Box `{BOX_NAME}`."));
    let cmds = controller.seen(|seen| {
        seen.requests
            .iter()
            .map(|r| matches!(r, Request::Start(_)))
            .collect::<Vec<_>>()
    });
    assert_eq!(cmds, [true, false], "a start, then a show");
}

#[test]
fn protocol_mismatch_fails_the_start_with_both_versions() {
    let mut rig = Rig::new();
    let newer = PROTOCOL + 1;
    let controller = rig.serve(newer);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    let posts = rig.slack.wait_posts(1);
    let text = &posts[0].text;
    assert!(
        text.starts_with(&format!("Could not start Box `{BOX_NAME}`")),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "client speaks protocol {PROTOCOL}, controller speaks {newer}"
        )),
        "{text}"
    );
    assert!(controller.seen(|seen| seen.requests.is_empty()));
}

#[test]
fn starting_row_left_by_a_stopped_daemon_is_settled_at_startup() {
    let mut rig = Rig::new();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    rig.wait_stderr("asking again");
    rig.kill_daemon();
    let controller = rig.serve(PROTOCOL);
    controller.script("show", names_box("show", BOX_NAME));
    rig.spawn_daemon();
    let posts = rig.slack.wait_posts(1);
    assert_eq!(posts[0].thread_ts, TS);
    assert_eq!(posts[0].text, format!("Started Box `{BOX_NAME}`."));
    let shows_only = controller.seen(|seen| {
        seen.requests.iter().all(|r| matches!(r, Request::Show(_))) && seen.requests.len() == 1
    });
    assert!(shows_only, "one show and no start after the restart");
}
