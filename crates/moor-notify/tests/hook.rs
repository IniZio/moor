//! `moor-notify` end to end against a scripted controller and a loopback receiver.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    clippy::zombie_processes,
    reason = "tests"
)]

use std::process::Command;

use moor_notify::config::DEFAULT_EVENTS;
use moor_notify::sign::signature;
use serde_json::{json, Value};

mod support;

use support::controller::{FakeController, Opened, TIP};
use support::receiver::{Posted, Receiver};
use support::{write_config, Front, KEY};

/// The identity the front asserts for the test user.
const ME: &str = "notify:a@example.com";

/// The body of `post` as JSON.
fn body(post: &Posted) -> Value {
    serde_json::from_slice(&post.body).unwrap()
}

/// Checks `post`'s signature the way the docs tell a receiver to.
fn assert_signed(post: &Posted) {
    let ts: u64 = post.headers["x-moor-timestamp"].parse().unwrap();
    assert_eq!(
        post.headers["x-moor-signature"],
        signature(KEY.as_bytes(), ts, &post.body),
        "signature over the timestamp and body"
    );
}

/// Waits until the front saved `cursor` for the test user. A front killed before it saves
/// posts that event again on its next start, which the docs tell receivers to expect.
fn wait_for_saved(state: &std::path::Path, cursor: i64) {
    for _ in 0..500 {
        let saved = std::fs::read(state.join("cursors.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        if saved.is_some_and(|v| v["a@example.com"] == cursor) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the front never saved cursor {cursor}");
}

#[test]
fn posts_signed_events_retries_and_resumes_after_a_restart() {
    let dir = moor_testkit::short_tempdir("notify").unwrap();
    let socket = dir.path().join("front");
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let controller = FakeController::serve(&socket);
    let receiver = Receiver::start(&[500]);
    let config = write_config(dir.path(), "A@example.com", &receiver.url, None);

    let front = Front::start(&socket, &config, &state);
    front.wait_for_line(&format!("from cursor {TIP}"));
    let kinds = Some(DEFAULT_EVENTS.map(str::to_owned).to_vec());
    let opened = |since, follow| Opened {
        since,
        follow,
        kinds: kinds.clone(),
        identity: Some(ME.to_owned()),
    };
    controller.wait_until("the live watch", |s| s.watches.len() == 2);
    assert_eq!(
        controller.seen(|s| s.watches.clone()),
        [opened(0, false), opened(TIP, true)],
        "a first start finds the tip, then follows from it"
    );
    assert_eq!(
        controller.seen(|s| s.requests.first().cloned()),
        Some(("enroll".to_owned(), Some(ME.to_owned())))
    );

    let idled = controller.emit("idled", json!({}));
    let failed = controller.emit("setup_failed", json!({"line": "make setup exited 2"}));
    let posted = receiver.wait_for(3);
    let deliveries: Vec<(u16, String)> = posted
        .iter()
        .map(|p| (p.status, p.headers["x-moor-delivery"].clone()))
        .collect();
    assert_eq!(
        deliveries,
        [
            (500, idled.to_string()),
            (200, idled.to_string()),
            (200, failed.to_string())
        ],
        "a 500 is posted again with the same delivery id, before the next event"
    );
    for post in &posted {
        assert_signed(post);
        assert_eq!(post.headers["content-type"], "application/json");
    }
    let first = body(&posted[1]);
    assert_eq!(first["schema"], "moor.notify/1");
    assert_eq!(first["event"], "idled");
    assert_eq!(first["box"], "app/fix");
    assert_eq!(first["box_id"], 7);
    assert_eq!(first["lifecycle"], "ready");
    assert_eq!(first["agent_said"]["text"], "Opened the PR.");
    assert_eq!(posted[1].headers["x-moor-event"], "idled");
    let second = body(&posted[2]);
    assert_eq!(second["event"], "setup_failed");
    assert_eq!(second["data"]["line"], "make setup exited 2");
    front.wait_for_line(&format!(
        "posted setup_failed of Box app/fix (delivery {failed})"
    ));
    wait_for_saved(&state, failed);
    drop(front);

    let capped = controller.emit("relaunch_capped", json!({"count": 3, "window_secs": 600}));
    let front = Front::start(&socket, &config, &state);
    front.wait_for_line(&format!("from cursor {failed}"));
    let posted = receiver.wait_for(4);
    assert_eq!(posted.len(), 4, "nothing posted twice after the restart");
    assert_eq!(posted[3].headers["x-moor-delivery"], capped.to_string());
    assert_eq!(body(&posted[3])["data"]["count"], 3);
    assert_eq!(
        controller.seen(|s| s.watches.last().cloned()),
        Some(opened(failed, true)),
        "a restart follows from the saved cursor, with no tip lookup"
    );
}

#[test]
fn a_refused_post_is_logged_and_the_next_event_still_goes() {
    let dir = moor_testkit::short_tempdir("notify").unwrap();
    let socket = dir.path().join("front");
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let controller = FakeController::serve(&socket);
    let receiver = Receiver::start(&[404]);
    let config = write_config(dir.path(), "a@example.com", &receiver.url, Some("[idled]"));
    let front = Front::start(&socket, &config, &state);
    front.wait_for_line("posting idled to http://127.0.0.1:");

    let refused = controller.emit("idled", json!({}));
    let taken = controller.emit("idled", json!({}));
    let posted = receiver.wait_for(2);
    assert_eq!(posted[0].headers["x-moor-delivery"], refused.to_string());
    assert_eq!(posted[1].headers["x-moor-delivery"], taken.to_string());
    front.wait_for_line(&format!(
        "a@example.com: gave up on idled of Box app/fix (delivery {refused}): HTTP 404, not retried"
    ));
}

#[test]
fn a_bad_config_stops_the_front_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(
        dir.path(),
        "a@example.com",
        "http://hooks.example.com/",
        None,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_moor-notify"))
        .env_clear()
        .args(["--socket", "/nonexistent"])
        .arg("--config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("hooks[0]: url:"), "{stderr}");
}
