//! What the thread's started message says besides the Box: how the work tree started and the
//! start's warnings.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use serde_json::{json, Value};

use moor_core::response::Response;

use super::{attempt_with, db, recorded_start, Attempt};

/// The thread reply of a start whose reply carries `extra` beside its Box.
fn reply_with(extra: &Value) -> String {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let mut value =
        json!({"box": {"id": 7, "name": "slack-10-5-1", "branch": "moor/slack-10-5-1"}});
    if let (Some(into), Some(from)) = (value.as_object_mut(), extra.as_object()) {
        into.extend(from.clone());
    }
    let seen = attempt_with(&db, &row, Attempt::First, &mut |_| {
        Ok(Response::ok("start", Some(value.clone())))
    });
    assert_eq!(seen.replies.len(), 1);
    seen.replies[0].2.clone()
}

const STARTED: &str = "Started Box `slack-10-5-1` on branch `moor/slack-10-5-1`.";

#[test]
fn the_started_message_says_how_the_work_tree_started() {
    for (tree, line) in [
        (json!({"state": "refreshed"}), "Work tree: refreshed"),
        (json!({"state": "cold"}), "Work tree: cold"),
        (
            json!({"state": "cold_after", "reason": "git fetch failed"}),
            "Work tree: cold after: git fetch failed",
        ),
    ] {
        let reply = reply_with(&json!({"work_tree": tree}));
        assert_eq!(reply, format!("{STARTED}\n{line}"));
    }
}

#[test]
fn the_started_message_carries_the_start_warnings_escaped() {
    let reply = reply_with(&json!({
        "work_tree": {"state": "cold"},
        "warnings_inline": ["agent dispatch failed: <!channel> no pane", "second"]
    }));
    assert_eq!(
        reply,
        format!(
            "{STARTED}\nWork tree: cold\n\
             Warning: agent dispatch failed: &lt;!channel&gt; no pane\nWarning: second"
        )
    );
}

#[test]
fn a_start_with_nothing_to_add_posts_the_plain_message() {
    assert_eq!(reply_with(&json!({"warnings_inline": []})), STARTED);
}
