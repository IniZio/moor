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

use moor_core::request::WebhookOutcome;
use moor_notify::sign::signature;
use serde_json::Value;

#[allow(
    dead_code,
    reason = "the real-controller test uses the rest of the front"
)]
mod support;

use support::controller::FakeController;
use support::receiver::{Posted, Receiver};
use support::Front;

/// The HMAC key the test webhooks sign with.
const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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

#[test]
fn posts_a_signed_delivery_and_reports_it_delivered() {
    let dir = moor_testkit::short_tempdir("notify").unwrap();
    let socket = dir.path().join("front");
    let controller = FakeController::serve(&socket);
    let receiver = Receiver::start(&[]);
    controller.queue(41, 1, &receiver.url, KEY, "idled");
    let _front = Front::start(&socket);

    let posted = receiver.wait_for(1);
    let post = &posted[0];
    assert_signed(post);
    assert_eq!(post.headers["content-type"], "application/json");
    assert_eq!(post.headers["x-moor-event"], "idled");
    assert_eq!(post.headers["x-moor-delivery"], "41");
    assert_eq!(body(post)["schema"], "moor.notify/1");
    assert_eq!(body(post)["event"], "idled");
    assert_eq!(body(post)["data"]["n"], 41);
    controller.wait_until("the report", |s| !s.reports.is_empty());
    assert_eq!(
        controller.seen(|s| s.reports.clone()),
        [(41, WebhookOutcome::Delivered)]
    );
    let (limit, lease_ms) = controller.seen(|s| s.claims[0]);
    assert!(limit > 0);
    assert!(lease_ms >= 600_000, "the lease outlasts the retries");
    assert!(!controller.seen(|s| s.asserted_identity));
}

#[test]
fn a_500_is_posted_again_and_then_reported_delivered() {
    let dir = moor_testkit::short_tempdir("notify").unwrap();
    let socket = dir.path().join("front");
    let controller = FakeController::serve(&socket);
    let receiver = Receiver::start(&[500]);
    controller.queue(7, 1, &receiver.url, KEY, "idled");
    let _front = Front::start(&socket);

    let posted = receiver.wait_for(2);
    let statuses: Vec<u16> = posted.iter().map(|p| p.status).collect();
    assert_eq!(statuses, [500, 200]);
    for post in &posted {
        assert_signed(post);
        assert_eq!(post.headers["x-moor-delivery"], "7");
    }
    assert_eq!(posted[0].body, posted[1].body, "the same bytes both times");
    controller.wait_until("the report", |s| !s.reports.is_empty());
    assert_eq!(
        controller.seen(|s| s.reports.clone()),
        [(7, WebhookOutcome::Delivered)]
    );
}

#[test]
fn a_410_is_reported_failed_without_retry_and_the_next_delivery_goes() {
    let dir = moor_testkit::short_tempdir("notify").unwrap();
    let socket = dir.path().join("front");
    let controller = FakeController::serve(&socket);
    let receiver = Receiver::start(&[410]);
    controller.queue(1, 1, &receiver.url, KEY, "idled");
    controller.queue(2, 1, &receiver.url, KEY, "setup_failed");
    let front = Front::start(&socket);

    controller.wait_until("both reports", |s| s.reports.len() == 2);
    front.wait_for_line("gave up on idled (delivery 1) to http://127.0.0.1:");
    let posted = receiver.posted();
    assert_eq!(posted.len(), 2, "the 410 is not posted again");
    assert_eq!(posted[0].headers["x-moor-delivery"], "1");
    assert_eq!(posted[1].headers["x-moor-delivery"], "2");
    let reports = controller.seen(|s| s.reports.clone());
    let WebhookOutcome::Failed { reason } = &reports[0].1 else {
        panic!("first report: {:?}", reports[0]);
    };
    assert!(reason.contains("410"), "{reason}");
    assert!(
        !reason.contains("127.0.0.1") && !reason.contains(KEY),
        "{reason}"
    );
    assert_eq!(reports[1], (2, WebhookOutcome::Delivered));
}

#[test]
fn deliveries_to_different_webhooks_go_out_together() {
    let dir = moor_testkit::short_tempdir("notify").unwrap();
    let socket = dir.path().join("front");
    let controller = FakeController::serve(&socket);
    let one = Receiver::start(&[]);
    let two = Receiver::start(&[]);
    controller.queue(1, 1, &one.url, KEY, "idled");
    controller.queue(2, 2, &two.url, KEY, "idled");
    let _front = Front::start(&socket);

    assert_signed(&one.wait_for(1)[0]);
    assert_signed(&two.wait_for(1)[0]);
    controller.wait_until("both reports", |s| s.reports.len() == 2);
}

#[test]
fn a_refused_claim_stops_the_front_with_the_controllers_message() {
    let dir = moor_testkit::short_tempdir("notify").unwrap();
    let socket = dir.path().join("front");
    let controller = FakeController::serve(&socket);
    controller.refuse_claims("this front needs `webhooks: true`");
    let out = Command::new(env!("CARGO_BIN_EXE_moor-notify"))
        .env_clear()
        .arg("--socket")
        .arg(&socket)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("needs `webhooks: true`"), "{stderr}");
}
