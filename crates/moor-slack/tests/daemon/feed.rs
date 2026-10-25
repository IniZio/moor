//! The feed: each Box keeps one status message in its thread, edited in place as the
//! controller's events arrive by Box id, and a finished turn or a close posts a fixed line.
//! A close queues a notice that waits for the starter's next message; a finished turn queues
//! a report that starts a coordinator turn on its own. A refold after a dropped watch or a
//! restart writes nothing twice.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::time::Duration;

use moor_core::response::Response;
use serde_json::json;

use super::inbox::reply;
use super::support::controller::{FakeController, Scripted};
use super::support::slack_fake::{Post, Update};
use super::support::{Rig, BOX_NAME, QUIET, USER};

/// The id the scripted controller gives the first Box it starts.
const BOX_ID: i64 = 1;
/// Longer than the 2 s the front waits between two edits of one status message.
const PAST_WINDOW: Duration = Duration::from_millis(2600);

/// The status message text of the first Box in state `state`.
fn status(state: &str) -> String {
    status_of_box(BOX_NAME, state)
}

/// The text of box `name`'s status message in `state`.
fn status_of_box(name: &str, state: &str) -> String {
    format!("*{name}*: {state}")
}

/// The line the front posts when the first Box's agent finishes its turn.
fn finished_line() -> String {
    format!("Box `{BOX_NAME}` finished its turn.")
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
    assert_eq!(post.text, status("starting (<1m)"));
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

/// The texts of the thread's notices, in the order they were queued.
fn notices(rig: &Rig) -> Vec<String> {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let mut stmt = db
        .prepare("SELECT text FROM slack_inbox WHERE kind IN ('notice', 'report') ORDER BY id")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.collect::<Result<_, _>>().unwrap()
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
    wait_status(&rig, &post.ts, &status("working (<1m)"));
    let question = json!({"question": "Which db?", "options": ["pg", "sqlite"]});
    controller.emit(BOX_ID, BOX_NAME, "needs_input", question);
    let want = status("waiting on a question (<1m): Which db?");
    wait_status(&rig, &post.ts, &want);
    assert_eq!(status_posts(&rig.slack.posts()).len(), 1, "posted once");
    wait_recorded(&rig, &post.ts, &want);
}

#[test]
fn a_working_box_silent_for_ten_minutes_has_a_stall_hint_in_its_status_edit() {
    let (rig, controller, _bin, post) = thread_with_status();
    controller.emit_ago(11 * 60, (BOX_ID, BOX_NAME, "working"), json!({}));
    controller.emit_ago(
        11 * 60,
        (BOX_ID, BOX_NAME, "session_started"),
        json!({"session": "s"}),
    );
    let want = format!(
        "{}\nNo event for 11m. It may be stuck.",
        status("working (11m)")
    );
    wait_status(&rig, &post.ts, &want);
    wait_recorded(&rig, &post.ts, &want);
}

#[test]
fn a_refold_after_a_dropped_watch_or_a_restart_writes_no_second_line_status_or_notice() {
    let (mut rig, controller, _bin, post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.wait_post(&finished_line());
    wait_status(&rig, &post.ts, &status("idle (<1m)"));
    std::thread::sleep(QUIET);
    let before = (
        rig.slack.posts().len(),
        rig.slack.updates().len(),
        notices(&rig),
    );

    let n = watches(&controller);
    controller.drop_watches();
    wait_refolded(&controller, n);
    let after = (
        rig.slack.posts().len(),
        rig.slack.updates().len(),
        notices(&rig),
    );
    assert_eq!(after, before, "a reconnect replays every event");

    let n = watches(&controller);
    rig.kill_daemon();
    rig.spawn_daemon();
    wait_refolded(&controller, n);
    let after = (
        rig.slack.posts().len(),
        rig.slack.updates().len(),
        notices(&rig),
    );
    assert_eq!(after, before, "a restart replays every event");
}

#[test]
fn a_backlog_read_in_parts_after_a_dropped_watch_is_acted_on_only_once_caught_up() {
    let (rig, controller, _bin, post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    wait_status(&rig, &post.ts, &status("idle (<1m)"));
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
    let want = status("working (<1m)");
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
    controller.emit(BOX_ID + 1, BOX_NAME, "idled", json!({}));
    std::thread::sleep(PAST_WINDOW);
    let updates = rig.slack.updates();
    assert!(
        updates.iter().all(|u| u.ts != post.ts),
        "another Box's events change nothing: {updates:?}"
    );
    let posts = rig.slack.posts();
    assert!(posts.iter().all(|p| p.text != finished_line()), "{posts:?}");
    controller.emit(BOX_ID, "another-name", "needs_input", json!({}));
    wait_status(&rig, &post.ts, &status("waiting on a question (<1m)"));
}

#[test]
fn a_box_closed_elsewhere_gets_its_final_status_a_closed_line_and_a_notice() {
    let (rig, controller, _bin, post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "closed", json!({"from": "idle"}));
    rig.wait_post(&format!("Box `{BOX_NAME}` closed."));
    wait_status(&rig, &post.ts, &status("closed"));
    let last = notices(&rig).pop();
    assert_eq!(last, Some(format!("[moor] Box `{BOX_NAME}` closed.")));
}

/// How many coordinator turns the front has taken; the mention that opened the thread is
/// the first.
fn turns(rig: &Rig) -> i64 {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db.query_row("SELECT COUNT(*) FROM slack_turns", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn an_idle_starts_one_turn_on_its_own_with_the_agents_last_message() {
    let (mut rig, controller, bin, _post) = thread_with_status();
    let said = "Fixed the parser.\nhttps://github.com/acme/demo/pull/7";
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "agent_said", json!({ "text": said }));
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.wait_post(&finished_line());
    rig.slack
        .wait_until("the summary turn", |_, _| turns(&rig) == 2);
    wait_turns_posted(&rig);
    let stdin = std::fs::read_to_string(bin.join("run-1/stdin")).unwrap();
    let report = format!(
        "[moor] Box `{BOX_NAME}` finished its turn. Its last message, in a Box quote:\n\
         > Fixed the parser.\n> https://github.com/acme/demo/pull/7\n"
    );
    assert!(stdin.contains(&report), "{stdin}");

    let n = watches(&controller);
    controller.drop_watches();
    wait_refolded(&controller, n);
    let n = watches(&controller);
    rig.kill_daemon();
    rig.spawn_daemon();
    wait_refolded(&controller, n);
    assert_eq!(
        turns(&rig),
        2,
        "a refold or a restart starts no second turn"
    );
    assert!(!bin.join("run-2").exists());
}

#[test]
fn an_idle_without_a_message_still_starts_a_turn_and_an_older_message_is_not_reused() {
    let (rig, controller, bin, _post) = thread_with_status();
    controller.emit(
        BOX_ID,
        BOX_NAME,
        "agent_said",
        json!({ "text": "old news" }),
    );
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.slack
        .wait_until("the first summary", |_, _| turns(&rig) == 2);
    wait_turns_posted(&rig);
    controller.emit(BOX_ID, BOX_NAME, "busy", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.slack
        .wait_until("the second summary", |_, _| turns(&rig) == 3);
    wait_turns_posted(&rig);
    let stdin = std::fs::read_to_string(bin.join("run-2/stdin")).unwrap();
    let new = stdin.rsplit("## New\n").next().unwrap();
    assert_eq!(new, format!("[moor] Box `{BOX_NAME}` finished its turn.\n"));
}

#[test]
fn each_finished_turn_of_one_box_posts_its_own_line_and_notice() {
    let (rig, controller, _bin, _post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.slack.wait_until("two finished lines", |posts, _| {
        posts.iter().filter(|p| p.text == finished_line()).count() == 2
    });
    std::thread::sleep(QUIET);
    let finished = format!("[moor] Box `{BOX_NAME}` finished its turn.");
    let queued = notices(&rig).into_iter().filter(|n| *n == finished).count();
    assert_eq!(queued, 2);
}

#[test]
fn a_refold_after_a_turn_used_the_notices_posts_no_second_line() {
    let (rig, controller, _bin, _post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.wait_post(&finished_line());
    reply(&rig, 2, USER, "what now");
    rig.wait_replies(2);
    wait_turns_posted(&rig);

    let n = watches(&controller);
    controller.drop_watches();
    wait_refolded(&controller, n);
    let posts = rig.slack.posts();
    let lines = posts.iter().filter(|p| p.text == finished_line()).count();
    assert_eq!(lines, 1, "{posts:?}");
}

#[test]
fn two_bound_boxes_in_one_thread_each_keep_their_own_status_and_lines() {
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
    let status_of = |name: &str, posts: &[Post]| {
        let head = format!("*{name}*: ");
        posts.iter().find(|p| p.text.starts_with(&head)).cloned()
    };
    let (posts, _) = rig.slack.wait_until("two status messages", |posts, _| {
        status_of(BOX_NAME, posts).is_some() && status_of(&second, posts).is_some()
    });
    let first_ts = status_of(BOX_NAME, &posts).unwrap().ts;
    let second_ts = status_of(&second, &posts).unwrap().ts;

    controller.emit(BOX_ID + 1, &second, "idled", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    rig.wait_post(&format!("Box `{second}` finished its turn."));
    wait_status(&rig, &second_ts, &status_of_box(&second, "idle (<1m)"));
    wait_status(&rig, &first_ts, &status("working (<1m)"));
    std::thread::sleep(QUIET);
    let posts = rig.slack.posts();
    assert!(posts.iter().all(|p| p.text != finished_line()), "{posts:?}");
}

#[test]
fn a_close_box_posts_one_closed_line_and_one_notice() {
    let (rig, controller, bin, post) = thread_with_status();
    Rig::tool_calls(&bin, None, &[("close_box", json!({"box": BOX_NAME}))]);
    controller.script("close", Scripted::Reply(Response::ok("close", None)));
    reply(&rig, 2, USER, "close it");
    rig.wait_post(&format!("Closed Box `{BOX_NAME}`."));
    rig.wait_box_state(BOX_NAME, "gone");
    controller.emit(BOX_ID, BOX_NAME, "closed", json!({"from": "idle"}));
    wait_status(&rig, &post.ts, &status("closed"));
    std::thread::sleep(QUIET);
    let posts = rig.slack.posts();
    let line = format!("Box `{BOX_NAME}` closed.");
    let closed: Vec<_> = posts.iter().filter(|p| p.text == line).collect();
    assert!(
        closed.is_empty(),
        "the close path already said it: {closed:?}"
    );
    let close_notices = notices(&rig)
        .into_iter()
        .filter(|n| n.ends_with("closed."))
        .count();
    assert_eq!(close_notices, 1);
}
