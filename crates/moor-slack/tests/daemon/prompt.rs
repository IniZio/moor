//! Needs-input prompts: a Box that waits on a question gets one message with a button per
//! option in its thread. Its requester answers with a button, or with a bare number while
//! the thread has exactly one open prompt; anything else reaches the coordinator.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::time::Duration;

use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::{json, Value};

use super::inbox::{reply, OTHER};
use super::support::controller::{FakeController, Scripted};
use super::support::slack_fake::{Click, Post};
use super::support::{Rig, BOX_NAME, CHANNEL, QUIET, TS, USER};

/// The id the scripted controller gives the first Box it starts.
const BOX_ID: i64 = 1;

/// The question the first Box asks.
fn asks() -> Value {
    json!({"question": "Proceed?", "options": ["Yes", "No"]})
}

/// The action ids of the buttons in `blocks`.
fn buttons(blocks: &Value) -> Vec<String> {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "actions")
        .flat_map(|b| b["elements"].as_array().cloned().unwrap_or_default())
        .filter_map(|e| e["action_id"].as_str().map(str::to_owned))
        .collect()
}

/// The prompt messages among `posts`: the only posts that carry blocks.
fn prompts(posts: &[Post]) -> Vec<Post> {
    posts
        .iter()
        .filter(|p| !p.blocks.is_null())
        .cloned()
        .collect()
}

/// Polls `slack.db` until it records `n` prompts; fails after ten seconds.
fn wait_recorded(rig: &Rig, n: i64) {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    for _ in 0..500 {
        let found: i64 = db
            .query_row("SELECT count(*) FROM slack_prompts", [], |r| r.get(0))
            .unwrap();
        if found >= n {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("slack.db never recorded {n} prompts");
}

/// Waits until the thread shows `n` prompts and `slack.db` records them; returns them.
fn wait_prompts(rig: &Rig, n: usize) -> Vec<Post> {
    let (posts, _) = rig
        .slack
        .wait_until("the prompts", |posts, _| prompts(posts).len() >= n);
    wait_recorded(rig, i64::try_from(n).unwrap());
    prompts(&posts)
}

/// A thread whose first Box is bound and asked [`asks`]; returns its prompt message.
fn asked() -> (Rig, FakeController, std::path::PathBuf, Post) {
    let (rig, controller, bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    controller.emit(BOX_ID, BOX_NAME, "needs_input", asks());
    let prompt = wait_prompts(&rig, 1).remove(0);
    (rig, controller, bin, prompt)
}

/// Presses button `choice` on `prompt` as `user`, with an envelope id derived from `n`.
fn click(rig: &Rig, n: u32, user: &str, prompt: &Post, choice: u32) {
    rig.slack.send_click(&Click {
        envelope_id: &format!("env-click-{n}"),
        channel: CHANNEL,
        user,
        message_ts: &prompt.ts,
        thread_ts: TS,
        value: &choice.to_string(),
        action_id: &format!("answer_{choice}"),
    });
}

/// The Box and menu choice of each `steer` the controller got that picks an option.
fn menus(controller: &FakeController) -> Vec<(String, u32)> {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::Steer(s) => Some((s.target.to_string(), s.menu?)),
                _ => None,
            })
            .collect()
    })
}

/// Answers the next `steer` as the controller does when the agent took it.
fn steer_ok(controller: &FakeController) {
    let ok = Response::ok("steer", Some(json!({})));
    controller.script("steer", Scripted::Reply(ok));
}

/// The text of every inbox message, in the order they were recorded.
fn messages(rig: &Rig) -> Vec<String> {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let mut stmt = db
        .prepare("SELECT text FROM slack_inbox WHERE kind = 'message' ORDER BY id")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

#[test]
fn a_needs_input_event_posts_one_prompt_with_buttons_for_the_requester() {
    let (rig, controller, _bin, prompt) = asked();
    assert_eq!(
        (prompt.channel.as_str(), prompt.thread_ts.as_str()),
        (CHANNEL, TS)
    );
    assert!(
        prompt.text.starts_with(&format!("<@{USER}> *{BOX_NAME}*")),
        "{}",
        prompt.text
    );
    assert!(prompt.text.contains("Proceed?"), "{}", prompt.text);
    assert_eq!(buttons(&prompt.blocks), ["answer_1", "answer_2"]);
    controller.emit(
        BOX_ID,
        BOX_NAME,
        "needs_input",
        json!({"question": "Again?", "options": ["A"]}),
    );
    let both = wait_prompts(&rig, 2);
    assert!(both[1].text.contains("Again?"), "{}", both[1].text);
    assert_eq!(buttons(&both[1].blocks), ["answer_1"]);
    std::thread::sleep(QUIET);
    assert_eq!(
        prompts(&rig.slack.posts()).len(),
        2,
        "one prompt per question"
    );
}

#[test]
fn a_refold_after_a_dropped_watch_or_a_restart_posts_no_second_prompt() {
    let (mut rig, controller, _bin, _prompt) = asked();
    let watches = controller.seen(|seen| seen.watches.len());
    controller.drop_watches();
    controller.wait_until("a new live watch", |seen| {
        seen.watches.iter().skip(watches).any(|(_, follow)| *follow)
    });
    std::thread::sleep(QUIET);
    rig.kill_daemon();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    let watches = controller.seen(|seen| seen.watches.len());
    controller.wait_until("a live watch after the restart", |seen| {
        seen.watches.iter().skip(watches).any(|(_, follow)| *follow)
    });
    std::thread::sleep(QUIET);
    assert_eq!(
        prompts(&rig.slack.posts()).len(),
        1,
        "{:?}",
        rig.slack.posts()
    );
}

#[test]
fn a_requester_click_on_a_live_prompt_steers_its_choice_and_shows_the_answer() {
    let (rig, controller, _bin, prompt) = asked();
    steer_ok(&controller);
    click(&rig, 1, USER, &prompt, 2);
    let (_, updates) = rig.slack.wait_until("the answered prompt", |_, updates| {
        updates.iter().any(|u| u.ts == prompt.ts)
    });
    assert_eq!(menus(&controller), [(BOX_NAME.to_owned(), 2)]);
    let edit = updates.iter().find(|u| u.ts == prompt.ts).unwrap();
    assert!(buttons(&edit.blocks).is_empty(), "{:?}", edit.blocks);
    assert_eq!(
        edit.blocks[1]["elements"][0]["text"],
        format!("Answered by <@{USER}>: 2. No")
    );
    assert!(rig.slack.ephemerals().is_empty());
}

#[test]
fn a_click_from_anyone_but_the_requester_gets_an_ephemeral_refusal_and_steers_nothing() {
    let (rig, controller, _bin, prompt) = asked();
    click(&rig, 1, OTHER, &prompt, 1);
    let ephemerals = rig.slack.wait_ephemerals(1);
    assert_eq!(ephemerals[0].user, OTHER);
    assert_eq!(
        (
            ephemerals[0].channel.as_str(),
            ephemerals[0].thread_ts.as_str()
        ),
        (CHANNEL, TS)
    );
    assert_eq!(
        ephemerals[0].text,
        format!("Only <@{USER}> can answer this prompt.")
    );
    std::thread::sleep(QUIET);
    assert!(menus(&controller).is_empty());
    assert!(rig.slack.updates().iter().all(|u| u.ts != prompt.ts));
}

#[test]
fn a_click_after_the_box_moved_on_is_refused_with_its_state() {
    let (rig, controller, _bin, prompt) = asked();
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    std::thread::sleep(QUIET);
    click(&rig, 1, USER, &prompt, 1);
    rig.wait_post(&format!(
        "This prompt is no longer open: {BOX_NAME} is now working."
    ));
    assert!(menus(&controller).is_empty());
}

#[test]
fn a_second_click_on_an_answered_prompt_is_refused_and_steers_once() {
    let (rig, controller, _bin, prompt) = asked();
    steer_ok(&controller);
    click(&rig, 1, USER, &prompt, 1);
    rig.slack.wait_until("the answered prompt", |_, updates| {
        updates.iter().any(|u| u.ts == prompt.ts)
    });
    click(&rig, 2, USER, &prompt, 2);
    let ephemerals = rig.slack.wait_ephemerals(1);
    assert_eq!(
        ephemerals[0].text,
        "This prompt was already answered: 1. Yes."
    );
    assert_eq!(menus(&controller), [(BOX_NAME.to_owned(), 1)]);
}

#[test]
fn a_refused_answer_is_posted_and_the_prompt_stays_open() {
    let (rig, controller, _bin, prompt) = asked();
    controller.script(
        "steer",
        Scripted::Reply(super::support::controller::refused(
            "steer",
            "Box is not waiting <!channel>",
        )),
    );
    click(&rig, 1, USER, &prompt, 1);
    rig.wait_post(&format!(
        "Box `{BOX_NAME}` refused the answer: Box is not waiting &lt;!channel&gt;"
    ));
    steer_ok(&controller);
    click(&rig, 2, USER, &prompt, 2);
    rig.slack.wait_until("the answered prompt", |_, updates| {
        updates.iter().any(|u| u.ts == prompt.ts)
    });
    assert_eq!(
        menus(&controller),
        [(BOX_NAME.to_owned(), 1), (BOX_NAME.to_owned(), 2)]
    );
}

#[test]
fn a_digit_from_the_requester_answers_the_one_open_prompt() {
    let (rig, controller, _bin, prompt) = asked();
    steer_ok(&controller);
    rig.mention(2, USER, "<@UBOT> 2", "1730000100.000002", Some(TS));
    rig.slack.wait_until("the answered prompt", |_, updates| {
        updates.iter().any(|u| u.ts == prompt.ts)
    });
    assert_eq!(menus(&controller), [(BOX_NAME.to_owned(), 2)]);
    assert_eq!(messages(&rig), ["fix the thing"], "the digit is not queued");
}

#[test]
fn a_digit_with_no_open_prompt_reaches_the_coordinator_as_text() {
    let (rig, controller, _bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    reply(&rig, 2, USER, "2");
    rig.slack.wait_acks(2);
    assert_eq!(messages(&rig), ["fix the thing", "2"]);
    assert!(menus(&controller).is_empty());
}

#[test]
fn a_digit_from_anyone_but_the_requester_answers_nothing() {
    let (rig, controller, _bin, _prompt) = asked();
    reply(&rig, 2, OTHER, "1");
    rig.wait_post(&format!("Only <@{USER}> can direct this thread."));
    std::thread::sleep(QUIET);
    assert!(menus(&controller).is_empty());
    assert_eq!(messages(&rig), ["fix the thing"]);
}

#[test]
fn a_digit_with_two_open_prompts_reaches_the_coordinator_as_text() {
    let (mut rig, controller, bin) = Rig::start_boxes();
    std::fs::OpenOptions::new()
        .append(true)
        .open(rig.path("slack.yaml"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"  max_open_boxes: 2\n"))
        .unwrap();
    rig.kill_daemon();
    rig.spawn_daemon();
    let goal = |g: &str| ("start_box", json!({ "goal": g }));
    Rig::tool_calls(
        &bin,
        Some(0),
        &[goal("fix the thing"), goal("and the other")],
    );
    rig.open_thread_with_a_box();
    let second = format!("{}2", BOX_NAME.strip_suffix('1').unwrap());
    rig.wait_box_state(&second, "bound");
    controller.emit(BOX_ID, BOX_NAME, "needs_input", asks());
    controller.emit(BOX_ID + 1, &second, "needs_input", asks());
    wait_prompts(&rig, 2);
    reply(&rig, 2, USER, "1");
    rig.slack.wait_acks(2);
    assert_eq!(messages(&rig), ["fix the thing", "1"]);
    std::thread::sleep(QUIET);
    assert!(menus(&controller).is_empty());
}

#[test]
fn a_prompt_slack_refuses_is_dropped_and_the_other_box_still_gets_its_lines() {
    let (mut rig, controller, bin) = Rig::start_boxes();
    std::fs::OpenOptions::new()
        .append(true)
        .open(rig.path("slack.yaml"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"  max_open_boxes: 2\n"))
        .unwrap();
    rig.kill_daemon();
    rig.spawn_daemon();
    let goal = |g: &str| ("start_box", json!({ "goal": g }));
    Rig::tool_calls(
        &bin,
        Some(0),
        &[goal("fix the thing"), goal("and the other")],
    );
    rig.open_thread_with_a_box();
    let second = format!("{}2", BOX_NAME.strip_suffix('1').unwrap());
    rig.wait_box_state(&second, "bound");
    rig.slack.refuse_block_posts(usize::MAX, "invalid_blocks");
    controller.emit(BOX_ID, BOX_NAME, "needs_input", asks());
    rig.slack
        .wait_until("the refused prompt", |posts, _| !prompts(posts).is_empty());
    controller.emit(BOX_ID + 1, &second, "idled", json!({}));
    rig.wait_post(&format!("Box `{second}` finished its turn."));
    std::thread::sleep(QUIET);
    assert_eq!(
        prompts(&rig.slack.posts()).len(),
        1,
        "a refused prompt is not retried"
    );
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let closed: Option<String> = db
        .query_row("SELECT closed_reason FROM slack_prompts", [], |r| r.get(0))
        .unwrap();
    assert!(closed.is_some(), "the refused prompt is recorded closed");
    rig.slack.refuse_block_posts(0, "invalid_blocks");
    controller.emit(
        BOX_ID,
        BOX_NAME,
        "needs_input",
        json!({"question": "Again?"}),
    );
    let posted = wait_prompts(&rig, 2);
    assert!(posted[1].text.contains("Again?"), "{}", posted[1].text);
}

#[test]
fn a_prompt_slack_fails_to_take_for_a_passing_reason_is_posted_on_a_retry() {
    let (rig, controller, _bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    rig.slack.refuse_block_posts(1, "internal_error");
    controller.emit(BOX_ID, BOX_NAME, "needs_input", asks());
    let (posts, _) = rig
        .slack
        .wait_until("the retried prompt", |posts, _| prompts(posts).len() >= 2);
    wait_recorded(&rig, 1);
    let posts = prompts(&posts);
    assert!(posts[1].text.contains("Proceed?"), "{}", posts[1].text);
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let (ts, closed): (Option<String>, Option<String>) = db
        .query_row(
            "SELECT message_ts, closed_reason FROM slack_prompts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(ts.is_some(), "the retried prompt is recorded posted");
    assert_eq!(closed, None, "the retried prompt stays open");
}
