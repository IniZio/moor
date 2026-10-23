//! Whom the front speaks for: every request it sends asserts the Slack user the request is
//! about, every watch it opens asserts one user, and a failing `auth.test` stops startup.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::collections::BTreeSet;
use std::time::Duration;

use moor_core::controller::PROTOCOL;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::inbox::reply;
use super::prompt::{asks, click, steer_ok, wait_prompts};
use super::support::controller::{FakeController, Scripted};
use super::support::slack_fake::{BOT_TOKEN, TEAM};
use super::support::{started_line, Rig, BOX_NAME, CHANNEL, MCP, QUIET, TS, USER};

/// The id the scripted controller gives the first Box it starts.
const BOX_ID: i64 = 1;
/// The other Slack user the two-user tests let start Boxes.
const SECOND: &str = "U0SECOND";
/// Thread of the second user's Box.
const SECOND_TS: &str = "1730000009.000300";
/// Name of the Box the second user's thread starts.
const SECOND_BOX: &str = "slack-1730000009-000300-1";

/// The identity the controller knows Slack user `user` by.
fn identity(user: &str) -> String {
    format!("slack:{TEAM}/{user}")
}

/// The wire command of `request`.
fn cmd(request: &moor_core::request::Request) -> String {
    serde_json::to_value(request).unwrap()["cmd"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn every_request_in_a_thread_that_starts_a_box_runs_a_turn_answers_and_closes_is_asserted() {
    let (rig, controller, bin) = Rig::start_boxes();
    let ask = [
        ("list_boxes", json!({})),
        ("close_box", json!({"box": BOX_NAME})),
    ];
    Rig::tool_calls(&bin, None, &ask);
    rig.open_thread_with_a_box();
    controller.emit(BOX_ID, BOX_NAME, "needs_input", asks());
    let prompt = wait_prompts(&rig, 1).remove(0);
    steer_ok(&controller);
    click(&rig, 1, USER, &prompt, 1);
    controller.wait_until("the answer", |seen| {
        let mut steers = seen.requests.iter();
        steers.any(|r| matches!(r, Request::Steer(s) if s.menu.is_some()))
    });
    let closed = Response::ok(
        "close",
        Some(json!({"box": {"id": 1, "lifecycle": "closed"}})),
    );
    controller.script("close", Scripted::Reply(closed));
    reply(&rig, 2, USER, "close it");
    rig.wait_post(&format!("Closed Box `{BOX_NAME}`."));
    let (cmds, acting) = controller.seen(|seen| {
        let cmds: BTreeSet<_> = seen.requests.iter().map(cmd).collect();
        (cmds, seen.acting.clone())
    });
    for want in ["start", "show", "steer", "close", "open-host-session"] {
        assert!(cmds.contains(want), "no {want} was sent: {cmds:?}");
    }
    let want = Some(identity(USER));
    assert!(acting.len() > 5, "{acting:?}");
    assert!(
        acting.iter().all(|who| *who == want),
        "a request was bare or for another user: {acting:?}"
    );
}

/// A daemon whose users [`USER`] and [`SECOND`] each own a thread with a bound Box.
fn two_users() -> (Rig, FakeController) {
    let mut rig = Rig::new();
    let bin = rig.use_fake_claude(None);
    rig.link_token(USER, MCP);
    rig.link_token(SECOND, MCP);
    Rig::tool_calls(
        &bin,
        None,
        &[("start_box", json!({"goal": "fix the thing"}))],
    );
    let controller = rig.serve(PROTOCOL);
    controller.keep_boxes();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.set_default_repo(super::support::REPO);
    rig.open_thread_with_a_box();
    rig.mention(2, SECOND, "<@UBOT> fix the other thing", SECOND_TS, None);
    rig.wait_post(&started_line(SECOND_BOX));
    rig.wait_box_state(SECOND_BOX, "bound");
    (rig, controller)
}

#[test]
fn each_slack_user_gets_one_watch_asserting_them_and_their_events_stay_in_their_thread() {
    let (rig, controller) = two_users();
    controller.wait_until("a live watch for each user", |seen| {
        let live = |who: &str| {
            let at = seen.watchers.iter().zip(&seen.watches);
            at.filter(|(w, (_, follow))| **w == Some(identity(who)) && *follow)
                .count()
        };
        live(USER) == 1 && live(SECOND) == 1
    });
    let watchers = controller.seen(|seen| seen.watchers.clone());
    assert!(
        watchers.iter().all(Option::is_some),
        "a bare watch: {watchers:?}"
    );
    let users: BTreeSet<_> = watchers.into_iter().flatten().collect();
    assert_eq!(users, BTreeSet::from([identity(USER), identity(SECOND)]));
    let idled = |rig: &Rig| {
        let line = format!("Box `{BOX_NAME}` finished its turn.");
        let posts = rig.slack.posts();
        posts
            .iter()
            .filter(|p| p.text == line)
            .map(|p| p.thread_ts.clone())
            .collect::<Vec<_>>()
    };
    controller.emit_for(&identity(SECOND), BOX_ID, BOX_NAME, "idled", json!({}));
    std::thread::sleep(QUIET);
    assert_eq!(idled(&rig), Vec::<String>::new(), "another user's watch");
    controller.emit_for(&identity(USER), BOX_ID, BOX_NAME, "idled", json!({}));
    rig.wait_post(&format!("Box `{BOX_NAME}` finished its turn."));
    std::thread::sleep(QUIET);
    assert_eq!(idled(&rig), [TS], "posted once, in the owner's thread only");
    let other = rig.slack.posts();
    let in_second = other.iter().filter(|p| p.thread_ts == SECOND_TS);
    assert!(
        in_second
            .filter(|p| p.channel == CHANNEL)
            .all(|p| !p.text.contains("finished")),
        "{other:?}"
    );
}

#[test]
fn a_failing_auth_test_stops_startup_and_names_it() {
    let mut rig = Rig::new();
    rig.slack.refuse_auth_test("invalid_auth");
    rig.spawn_daemon();
    let status = rig.wait_exit(Duration::from_secs(5));
    assert!(status.is_some_and(|s| !s.success()), "{status:?}");
    let stderr = rig.stderr();
    assert!(stderr.contains("auth.test"), "{stderr}");
    assert!(!stderr.contains(BOT_TOKEN), "{stderr}");
}

#[test]
fn startup_reads_the_team_from_auth_test_once() {
    let (rig, _controller) = Rig::start();
    let asked = |rig: &Rig| {
        rig.slack
            .requests()
            .iter()
            .filter(|r| r.path == "/auth.test")
            .map(|r| r.auth.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(asked(&rig), [format!("Bearer {BOT_TOKEN}")]);
}
