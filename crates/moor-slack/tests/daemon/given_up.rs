//! A close moor gave up: one line in the thread that says why, one more once moor put the Box
//! back to sleep, and a notice for the coordinator. A refold posts neither line again.

use std::time::Duration;

use serde_json::json;

use super::feed::{notices, thread_with_status, wait_refolded, watches};
use super::support::{BOX_NAME, QUIET};

/// The id the scripted controller gives the first Box it starts.
const BOX_ID: i64 = 1;

#[test]
fn a_close_moor_gave_up_posts_why_then_that_the_box_is_asleep_again_once_each() {
    let (rig, controller, _bin, _post) = thread_with_status();
    let refused = json!({"reason": {"refusal": "unsaved", "branch": "b", "ahead": 2, "dirty": 1},
        "error": "the Box holds work"});
    controller.emit(BOX_ID, BOX_NAME, "close_refused", refused);
    controller.emit(BOX_ID, BOX_NAME, "asleep", json!({"auto": false}));
    let why = format!(
        "Box `{BOX_NAME}` is still open: it holds work that is not saved anywhere (2 unpushed \
         commits and 1 uncommitted file), so its close was not finished. Nothing is lost unless \
         you confirm. If you still want it closed, say so in this thread."
    );
    let asleep = format!("Box `{BOX_NAME}` is asleep again. Reply here to wake it.");
    rig.wait_post(&why);
    rig.wait_post(&asleep);
    let n = watches(&controller);
    controller.drop_watches();
    wait_refolded(&controller, n);
    std::thread::sleep(QUIET + Duration::from_millis(500));
    let posts = rig.slack.posts();
    assert_eq!(
        posts.iter().filter(|p| p.text == why).count(),
        1,
        "{posts:?}"
    );
    assert_eq!(
        posts.iter().filter(|p| p.text == asleep).count(),
        1,
        "{posts:?}"
    );
    let notices = notices(&rig);
    let given_up = notices
        .iter()
        .filter(|n| n.contains("gave up a close"))
        .count();
    assert_eq!(given_up, 1, "{notices:?}");
}

#[test]
fn an_asked_sleep_with_no_close_given_up_before_it_posts_no_asleep_again_line() {
    let (rig, controller, _bin, _post) = thread_with_status();
    controller.emit(BOX_ID, BOX_NAME, "asleep", json!({"auto": false}));
    controller.emit(BOX_ID, BOX_NAME, "closed", json!({"from": "idle"}));
    rig.wait_post(&format!("Box `{BOX_NAME}` closed."));
    let posts = rig.slack.posts();
    assert!(
        posts.iter().all(|p| !p.text.contains("asleep again")),
        "{posts:?}"
    );
}
