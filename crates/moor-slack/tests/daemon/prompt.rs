//! Needs-input prompts: a Box that waits on a question gets one message with a button per
//! option in its thread, and a refold posts it once.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::time::Duration;

use serde_json::{json, Value};

use super::support::controller::FakeController;
use super::support::slack_fake::Post;
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
    rig.slack.refuse_block_posts(usize::MAX);
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
    rig.slack.refuse_block_posts(0);
    controller.emit(
        BOX_ID,
        BOX_NAME,
        "needs_input",
        json!({"question": "Again?"}),
    );
    let posted = wait_prompts(&rig, 2);
    assert!(posted[1].text.contains("Again?"), "{}", posted[1].text);
}
