//! A Box whose agent session ended without a clean exit never reads "finished its turn": its
//! thread gets one plain line and the coordinator a report that says the same.

use serde_json::json;

use super::feed::wait_turns_posted;
use super::feed::{finished_line, notices, thread_with_status, turns, wait_refolded, watches};
use super::support::{BOX_NAME, QUIET};

const BOX_ID: i64 = 1;

fn stopped_line(exit: &str, lost: bool) -> String {
    let lost = if lost {
        ", and its transcript could not be saved"
    } else {
        ""
    };
    format!(
        "`{BOX_NAME}`'s agent stopped unexpectedly ({exit}){lost}. Its work so far is still in \
         the Box. Reply here and I can restart it."
    )
}

fn ended(exit: i32) -> serde_json::Value {
    json!({ "exit": exit, "session": 24, "cause": "exited" })
}

#[test]
fn an_agent_that_exited_with_255_posts_the_stop_line_and_never_the_finished_line() {
    let (mut rig, controller, bin, post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "session_ended", ended(255));
    controller.emit(
        BOX_ID,
        BOX_NAME,
        "transcript_skipped",
        json!({ "reason": "nothing to archive", "trigger": "exit" }),
    );
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.wait_post(&stopped_line("exit 255", false));
    rig.slack
        .wait_until("the report turn", |_, _| turns(&rig) == 2);
    wait_turns_posted(&rig);
    std::thread::sleep(QUIET);
    let posts = rig.slack.posts();
    let line = posts
        .iter()
        .find(|p| p.text == stopped_line("exit 255", false))
        .unwrap();
    assert_eq!(line.thread_ts, post.thread_ts, "in the Box's own thread");
    assert!(posts.iter().all(|p| p.text != finished_line()), "{posts:?}");
    let stdin = std::fs::read_to_string(bin.join("run-1/stdin")).unwrap();
    let report = format!("[moor] {}\n", stopped_line("exit 255", false));
    assert!(stdin.contains(&report), "{stdin}");
    assert!(!stdin.contains("finished its turn"), "{stdin}");

    let n = watches(&controller);
    rig.kill_daemon();
    rig.spawn_daemon();
    wait_refolded(&controller, n);
    let count = |rig: &super::support::Rig| {
        let want = stopped_line("exit 255", false);
        rig.slack.posts().iter().filter(|p| p.text == want).count()
    };
    assert_eq!(count(&rig), 1, "a restart posts no second line");
    assert_eq!(turns(&rig), 2, "a restart starts no second turn");
}

#[test]
fn a_clean_exit_keeps_the_finished_line() {
    let (rig, controller, _bin, _post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "session_ended", ended(0));
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.wait_post(&finished_line());
    std::thread::sleep(QUIET);
    let all = rig.slack.posts();
    assert!(all.iter().all(|p| !p.text.contains("stopped")), "{all:?}");
}

#[test]
fn a_failed_transcript_save_adds_its_suffix_whichever_event_comes_first() {
    let (rig, controller, _bin, _post) = thread_with_status();
    let failed = json!({ "trigger": "exit", "error": "scp failed" });
    controller.emit(BOX_ID, BOX_NAME, "session_ended", ended(255));
    controller.emit(BOX_ID, BOX_NAME, "transcript_failed", failed.clone());
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.wait_post(&stopped_line("exit 255", true));
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    controller.emit(BOX_ID, BOX_NAME, "transcript_failed", failed);
    controller.emit(BOX_ID, BOX_NAME, "session_ended", ended(1));
    controller.emit(BOX_ID, BOX_NAME, "idled", json!({}));
    rig.wait_post(&stopped_line("exit 1", true));
    std::thread::sleep(QUIET);
    let reports = notices(&rig);
    let want = format!("[moor] {}", stopped_line("exit 255", true));
    assert!(reports.contains(&want), "{reports:?}");
}
