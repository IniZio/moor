//! The feed: each Box keeps one status message in its thread, edited in place as the
//! controller's events arrive by Box id. A refold after a dropped watch or a restart posts
//! no message twice and edits none to the text it already shows.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::time::Duration;

use serde_json::json;

use super::support::controller::FakeController;
use super::support::slack_fake::{Post, Update};
use super::support::{Rig, BOX_NAME, QUIET};

/// The id the scripted controller gives the first Box it starts.
const BOX_ID: i64 = 1;
/// Longer than the 2 s the front waits between two edits of one status message.
const PAST_WINDOW: Duration = Duration::from_millis(2600);

/// The status message text of the first Box in state `state`.
fn status(state: &str) -> String {
    format!("*{BOX_NAME}*: {state}")
}

/// Posts of the first Box's status message.
fn status_posts(posts: &[Post]) -> Vec<&Post> {
    let head = format!("*{BOX_NAME}*:");
    posts.iter().filter(|p| p.text.starts_with(&head)).collect()
}

/// Opens a thread whose first Box is bound, and waits for its status message.
fn thread_with_status() -> (Rig, FakeController, std::path::PathBuf, Post) {
    let (rig, controller, bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    let (posts, _) = rig.slack.wait_until("the status message", |posts, _| {
        !status_posts(posts).is_empty()
    });
    let post = status_posts(&posts)[0].clone();
    assert_eq!(post.text, status("starting"));
    (rig, controller, bin, post)
}

/// Waits until the status message `ts` is edited to `text`.
fn wait_status(rig: &Rig, ts: &str, text: &str) -> Vec<Update> {
    let (_, updates) = rig.slack.wait_until(text, |_, updates| {
        updates.iter().any(|u| u.ts == ts && u.text == text)
    });
    updates
}

/// Polls `slack.db` until the first Box's status message is recorded as `ts` showing `text`;
/// fails after ten seconds.
fn wait_recorded(rig: &Rig, ts: &str, text: &str) {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let recorded = || -> (Option<String>, Option<String>) {
        db.query_row(
            "SELECT status_ts, status_text FROM slack_boxes WHERE box_name = ?1",
            [BOX_NAME],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    let want = (Some(ts.to_owned()), Some(text.to_owned()));
    for _ in 0..500 {
        if recorded() == want {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(recorded(), want);
}

/// How many watches the controller has served so far.
fn watches(controller: &FakeController) -> usize {
    controller.seen(|seen| seen.watches.len())
}

/// Waits until the front has dialled a live watch after the `n`th watch, so its last
/// backlog is folded, then gives the feed time to act on it.
fn wait_refolded(controller: &FakeController, n: usize) {
    controller.wait_until("a new live watch", |seen| {
        seen.watches.iter().skip(n).any(|(_, follow)| *follow)
    });
    std::thread::sleep(QUIET);
}

#[test]
fn each_box_keeps_one_status_message_edited_in_place_and_never_posted_twice() {
    let (rig, controller, _bin, post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    wait_status(&rig, &post.ts, &status("working"));
    let question = json!({"question": "Which db?", "options": ["pg", "sqlite"]});
    controller.emit(BOX_ID, BOX_NAME, "needs_input", question);
    let want = status("needs input: Which db?");
    wait_status(&rig, &post.ts, &want);
    assert_eq!(status_posts(&rig.slack.posts()).len(), 1, "posted once");
    wait_recorded(&rig, &post.ts, &want);
}

#[test]
fn a_refold_after_a_dropped_watch_or_a_restart_posts_and_edits_nothing() {
    let (mut rig, controller, _bin, post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    wait_status(&rig, &post.ts, &status("idle"));
    std::thread::sleep(QUIET);
    let before = (rig.slack.posts().len(), rig.slack.updates().len());

    let n = watches(&controller);
    controller.drop_watches();
    wait_refolded(&controller, n);
    let after = (rig.slack.posts().len(), rig.slack.updates().len());
    assert_eq!(after, before, "a reconnect replays every event");

    let n = watches(&controller);
    rig.kill_daemon();
    rig.spawn_daemon();
    wait_refolded(&controller, n);
    let after = (rig.slack.posts().len(), rig.slack.updates().len());
    assert_eq!(after, before, "a restart replays every event");
}

#[test]
fn a_backlog_read_in_parts_after_a_dropped_watch_is_acted_on_only_once_caught_up() {
    let (rig, controller, _bin, post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    wait_status(&rig, &post.ts, &status("idle"));
    std::thread::sleep(PAST_WINDOW);
    let before = rig.slack.updates().len();

    controller.split_backlogs(1, QUIET);
    let n = watches(&controller);
    controller.drop_watches();
    wait_refolded(&controller, n);
    let updates = rig.slack.updates();
    assert_eq!(
        updates.len(),
        before,
        "edited midway through a replay: {updates:?}"
    );
}

/// Waits until every coordinator turn's line is posted, so a later `chat.update` is the feed's.
fn wait_turns_posted(rig: &Rig) {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let sql = "SELECT COUNT(*) FROM slack_turns WHERE posted_at IS NULL";
    rig.slack.wait_until("every turn posted", |_, _| {
        db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap() == 0
    });
}

#[test]
fn a_status_edit_slack_refuses_is_sent_again_after_the_window() {
    let (rig, controller, _bin, post) = thread_with_status();
    wait_turns_posted(&rig);
    let refused = json!({"ok": false, "error": "internal_error"});
    rig.slack.answer_updates(&[(200, refused)]);
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    let want = status("working");
    rig.slack.wait_until("the edit sent again", |_, updates| {
        let tries = updates.iter().filter(|u| u.ts == post.ts && u.text == want);
        tries.count() == 2
    });
    wait_recorded(&rig, &post.ts, &want);
}

#[test]
fn events_reach_a_box_by_its_id_and_its_name_is_only_for_display() {
    let (rig, controller, _bin, post) = thread_with_status();
    controller.emit(BOX_ID + 1, BOX_NAME, "working", json!({}));
    std::thread::sleep(PAST_WINDOW);
    let updates = rig.slack.updates();
    assert!(
        updates.iter().all(|u| u.ts != post.ts),
        "another Box's events change nothing: {updates:?}"
    );
    controller.emit(BOX_ID, "another-name", "needs_input", json!({}));
    wait_status(&rig, &post.ts, &status("needs input"));
}

#[test]
fn a_box_closed_elsewhere_gets_its_final_status() {
    let (rig, controller, _bin, post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "closed", json!({"from": "idle"}));
    wait_status(&rig, &post.ts, &status("closed"));
}
