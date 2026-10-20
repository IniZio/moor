//! Smoke tests against a real `moor controller serve` with no repos, reached only through
//! its socket.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::time::Duration;

use super::support::{Rig, BOX_NAME, TS, USER};

/// The refusal the controller gives for any repo when none are configured.
const UNKNOWN: &str = "repo nosuch is not known to the controller; no repos are configured";

#[test]
fn real_controller_refusal_is_posted_in_the_thread() {
    let mut rig = Rig::new();
    let _controller = rig.real_controller();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> start nosuch fix the thing", TS, None);
    let posts = rig.slack.wait_posts(1);
    assert_eq!(posts[0].thread_ts, TS);
    assert_eq!(
        posts[0].text,
        format!("Could not start Box `{BOX_NAME}`: {UNKNOWN}")
    );
}

/// The second mention arrives while the controller is down, so its start fails to connect
/// and the worker asks again once the controller is back.
#[test]
fn real_controller_restart_is_survived() {
    let mut rig = Rig::new();
    let first = rig.real_controller();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> start nosuch fix the thing", TS, None);
    rig.slack.wait_posts(1);
    drop(first);
    let ts = "1730000009.000300";
    rig.mention(2, USER, "<@UBOT> start nosuch fix the thing", ts, None);
    rig.slack.wait_acks(2);
    std::thread::sleep(Duration::from_millis(500));
    let _second = rig.real_controller();
    let posts = rig.slack.wait_posts(2);
    assert_eq!(posts[1].thread_ts, ts);
    assert!(posts[1].text.ends_with(UNKNOWN), "{}", posts[1].text);
    let stderr = rig.stderr();
    assert!(
        stderr.contains("asking again"),
        "the start met a stopped controller: {stderr}"
    );
}
