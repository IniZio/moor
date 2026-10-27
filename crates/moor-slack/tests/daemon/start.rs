//! Starting a Box through the coordinator's `start_box`, against the scripted controller: a
//! top-level mention opens a thread, its turn calls `start_box`, and the worker starts the
//! Box with the reconcile it always had.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::Path;

use moor_core::controller::PROTOCOL;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::support::controller::{FakeController, Scripted};
use super::support::slack_fake::{Mention, APP_TOKEN, BOT_TOKEN};
use super::support::{started_line, Rig, BOX_NAME, CHANNEL, MCP, QUIET, REPO, TS, USER};

/// Ts of a second thread.
const TS2: &str = "1730000005.000200";
/// The first Box of the second thread.
const BOX2: &str = "slack-1730000005-000200-1";
/// The host Session the scripted controller opens first.
const HOST: i64 = 100;

/// A `start` or `show` reply that names Box 7 called `name`.
fn names_box(cmd: &str, name: &str) -> Scripted {
    let value = json!({"box": {"id": 7, "name": name, "branch": format!("moor/{name}")}});
    Scripted::Reply(Response::ok(cmd, Some(value)))
}

/// The name, repo, prompt and parent of one `start`.
type Start = (String, Option<String>, Option<String>, Option<i64>);

/// The name, repo, prompt and parent of every `start` the controller received.
fn starts(controller: &FakeController) -> Vec<Start> {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::Start(s) => Some((
                    s.name.as_str().to_owned(),
                    s.repo.as_ref().map(ToString::to_string),
                    s.prompt.clone(),
                    s.parent.map(moor_core::name::SessionId::get),
                )),
                _ => None,
            })
            .collect()
    })
}

/// The `start` the first thread's turn asks for.
fn first_start() -> Start {
    (
        BOX_NAME.to_owned(),
        Some(REPO.to_owned()),
        Some("fix the thing".to_owned()),
        Some(HOST),
    )
}

/// Whether each request after the hello is a `start` (true) or the worker's own `show`
/// (false); the turn's `show` of a running Box and host Session requests are left out.
fn starts_and_shows(controller: &FakeController) -> Vec<bool> {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::Start(_) => Some(true),
                Request::Show(s) if !s.agent => Some(false),
                _ => None,
            })
            .collect()
    })
}

/// The texts of the posts that say a Box started or could not.
fn start_lines(rig: &Rig) -> Vec<(String, String)> {
    let posts = rig.slack.posts();
    posts
        .into_iter()
        .filter(|p| p.text.contains("start Box") || p.text.starts_with("Started Box"))
        .map(|p| (p.thread_ts, p.text))
        .collect()
}

/// Makes every turn of the fake in `bin` call `start_box` for `fix the thing`.
fn every_turn_starts(bin: &Path) {
    Rig::tool_calls(
        bin,
        None,
        &[("start_box", json!({"prompt": "fix the thing"}))],
    );
}

#[test]
fn allowed_mention_starts_a_box_and_replies_in_the_thread() {
    let (rig, controller, _bin) = Rig::start_boxes();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_post(&started_line(BOX_NAME));
    assert_eq!(rig.slack.wait_acks(1), ["env-1"]);
    assert_eq!(start_lines(&rig), [(TS.to_owned(), started_line(BOX_NAME))]);
    assert_eq!(
        starts(&controller),
        [first_start()],
        "the host Session is the parent"
    );
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
    let (rig, controller, _bin) = Rig::start_boxes();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.slack.send_mention(&Mention {
        event_id: "Ev1",
        envelope_id: "env-1-retry",
        channel: CHANNEL,
        user: USER,
        text: "<@UBOT> fix the thing",
        ts: TS,
        thread_ts: None,
    });
    assert_eq!(rig.slack.wait_acks(2), ["env-1", "env-1-retry"]);
    rig.wait_post(&started_line(BOX_NAME));
    std::thread::sleep(QUIET);
    assert_eq!(start_lines(&rig).len(), 1, "{:?}", rig.slack.posts());
    assert_eq!(starts(&controller).len(), 1);
}

#[test]
fn second_mention_in_a_thread_starts_nothing_new() {
    let (rig, controller, bin) = Rig::start_boxes();
    every_turn_starts(&bin);
    rig.open_thread_with_a_box();
    rig.mention(
        2,
        USER,
        "<@UBOT> and another thing",
        "1730000005.000300",
        Some(TS),
    );
    rig.wait_replies(2);
    let refused = Rig::tool_results(&bin, 1);
    let want = "This thread already has 1 open Box(es), its limit. Close one first.";
    assert_eq!(refused, [(want.to_owned(), true)]);
    assert_eq!(starts(&controller).len(), 1);
}

#[test]
fn with_the_limit_at_2_a_second_start_works_and_a_third_is_refused() {
    let mut rig = Rig::new();
    let bin = rig.use_fake_claude(None);
    std::fs::OpenOptions::new()
        .append(true)
        .open(rig.path("slack.yaml"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"  max_open_boxes: 2\n"))
        .unwrap();
    rig.link_token(USER, MCP);
    let controller = rig.serve(PROTOCOL);
    controller.keep_boxes();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.set_default_repo(REPO);
    let start = |prompt: &str| ("start_box", json!({"prompt": prompt}));
    Rig::tool_calls(&bin, None, &[start("one"), start("two"), start("three")]);
    rig.mention(1, USER, "<@UBOT> three things", TS, None);
    rig.wait_replies(1);
    let results = Rig::tool_results(&bin, 0);
    let errors: Vec<bool> = results.iter().map(|r| r.1).collect();
    assert_eq!(errors, [false, false, true], "{results:?}");
    rig.wait_post(&started_line("slack-1730000000-000100-2"));
    let mut names: Vec<_> = starts(&controller).into_iter().map(|s| s.0).collect();
    names.sort();
    assert_eq!(names, [BOX_NAME, "slack-1730000000-000100-2"]);
}

#[test]
fn tokens_never_reach_stderr() {
    let (rig, controller, _bin) = Rig::start_boxes();
    controller.script("start", Scripted::Hangup);
    controller.script("show", names_box("show", BOX_NAME));
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_post(&started_line(BOX_NAME));
    let stderr = rig.stderr();
    assert!(
        stderr.contains(BOX_NAME),
        "the lost start is logged: {stderr}"
    );
    assert!(!stderr.contains(APP_TOKEN), "{stderr}");
    assert!(!stderr.contains(BOT_TOKEN), "{stderr}");
    assert!(!stderr.contains(MCP), "{stderr}");
}

#[test]
fn start_lost_mid_request_is_found_with_show() {
    let (rig, controller, _bin) = Rig::start_boxes();
    controller.script("start", Scripted::Hangup);
    controller.script("show", names_box("show", BOX_NAME));
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_post(&started_line(BOX_NAME));
    assert_eq!(
        starts_and_shows(&controller),
        [true, false],
        "a start, then a show"
    );
}

#[test]
fn protocol_mismatch_stops_the_turn_before_any_start_with_both_versions() {
    let mut rig = Rig::new();
    rig.use_fake_claude(None);
    rig.link_token(USER, MCP);
    let newer = PROTOCOL + 1;
    let controller = rig.serve(newer);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    let posts = rig.slack.wait_posts(1);
    let text = &posts[0].text;
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
    let (mut rig, controller, _bin) = Rig::start_boxes();
    controller.script("start", Scripted::Hold);
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    controller.wait_until("the held start", |seen| {
        seen.requests.iter().any(|r| matches!(r, Request::Start(_)))
    });
    rig.kill_daemon();
    controller.script("show", names_box("show", BOX_NAME));
    rig.spawn_daemon();
    rig.wait_post(&started_line(BOX_NAME));
    assert_eq!(
        starts_and_shows(&controller),
        [true, false],
        "one show and no second start after the restart"
    );
}

#[test]
fn start_sent_again_after_a_restart_carries_the_stored_prompt() {
    let (mut rig, controller, _bin) = Rig::start_boxes();
    controller.script("start", Scripted::Hold);
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    controller.wait_until("the held start", |seen| {
        seen.requests.iter().any(|r| matches!(r, Request::Start(_)))
    });
    rig.kill_daemon();
    rig.spawn_daemon();
    rig.wait_post(&started_line(BOX_NAME));
    assert_eq!(starts(&controller), [first_start(), first_start()]);
    assert_eq!(controller.boxes(), [BOX_NAME]);
}

#[test]
fn mention_acked_while_the_worker_waits_survives_a_crash() {
    let (mut rig, controller, bin) = Rig::start_boxes();
    every_turn_starts(&bin);
    controller.script("start", Scripted::Hold);
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    controller.wait_until("the held start", |seen| {
        seen.requests.iter().any(|r| matches!(r, Request::Start(_)))
    });
    rig.mention(2, USER, "<@UBOT> fix the other thing", TS2, None);
    rig.slack.wait_acks(2);
    rig.kill_daemon();
    rig.spawn_daemon();
    rig.wait_post(&started_line(BOX2));
    std::thread::sleep(QUIET);
    let second: Vec<_> = starts(&controller)
        .into_iter()
        .filter(|s| s.0 == BOX2)
        .collect();
    let want = (
        BOX2.to_owned(),
        Some(REPO.to_owned()),
        Some("fix the thing".to_owned()),
        Some(HOST + 1),
    );
    assert_eq!(second, [want], "the second thread's Box starts once");
}

#[test]
fn starts_lost_with_their_connection_run_once_the_controller_answers() {
    let (rig, controller, bin) = Rig::start_boxes();
    every_turn_starts(&bin);
    controller.script("start", Scripted::Hangup);
    controller.script("start", Scripted::Hangup);
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.mention(2, USER, "<@UBOT> fix the other thing", TS2, None);
    rig.wait_post(&started_line(BOX_NAME));
    rig.wait_post(&started_line(BOX2));
    let mut boxes = controller.boxes();
    boxes.sort();
    assert_eq!(boxes, [BOX_NAME, BOX2], "one Box per thread");
    std::thread::sleep(QUIET);
    assert_eq!(start_lines(&rig).len(), 2, "each start is posted once");
}

#[test]
fn a_thread_the_controller_cannot_answer_holds_up_no_other_thread() {
    let (rig, controller, bin) = Rig::start_boxes();
    every_turn_starts(&bin);
    controller.script("start", Scripted::Hangup);
    controller.script("show", Scripted::Hangup);
    controller.script("show", Scripted::Hangup);
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_stderr(&format!("start of {BOX_NAME}"));
    rig.mention(2, USER, "<@UBOT> fix the other thing", TS2, None);
    rig.wait_post(&started_line(BOX_NAME));
    let threads: Vec<_> = start_lines(&rig).into_iter().map(|(t, _)| t).collect();
    assert_eq!(threads, [TS2, TS], "the second thread is not held up");
}

#[test]
fn a_rerun_after_a_killed_start_box_refuses_a_second_start_and_names_the_box() {
    let (mut rig, controller, bin) = Rig::start_boxes();
    every_turn_starts(&bin);
    std::fs::write(bin.join("hang-after-tools"), "").unwrap();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_post(&started_line(BOX_NAME));
    rig.kill_daemon();
    std::fs::remove_file(bin.join("hang-after-tools")).unwrap();
    rig.spawn_daemon();
    rig.wait_replies(1);
    let refused = Rig::tool_results(&bin, 1);
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(refused[0].1, "{refused:?}");
    let named =
        format!("Your last turn already started Box `{BOX_NAME}`, so no second one starts.");
    assert!(refused[0].0.starts_with(&named), "{refused:?}");
    assert_eq!(starts(&controller).len(), 1);
}
