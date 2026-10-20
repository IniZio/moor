//! Startup: the daemon reaches Slack and the controller in any start order, reads token
//! files under a leading `~` from home, and refuses token files that others can read.

use std::os::unix::fs::PermissionsExt as _;
use std::time::Duration;

use moor_core::controller::PROTOCOL;

use super::support::controller::BACKLOG_END;
use super::support::slack_fake::{APP_TOKEN, BOT_TOKEN};
use super::support::{Rig, TS, USER};

#[test]
fn connects_to_slack_and_follows_the_controller() {
    let (rig, controller) = Rig::start();
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    assert_eq!(rig.slack.wait_acks(1), ["env-1"]);
    assert!(rig
        .slack
        .requests()
        .iter()
        .filter(|r| r.path == "/apps.connections.open")
        .all(|r| r.auth == format!("Bearer {APP_TOKEN}")));
    controller.wait_until("a backlog watch, then a live one from its cursor", |seen| {
        seen.watches == [(0, false), (BACKLOG_END, true)]
    });
    assert_eq!(controller.seen(|seen| seen.hellos.clone()), [PROTOCOL; 2]);
}

#[test]
fn dials_a_controller_that_starts_later() {
    let mut rig = Rig::new();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    std::thread::sleep(Duration::from_millis(600));
    let stderr = rig.stderr();
    assert!(stderr.contains("controller: connect to"), "{stderr}");
    let controller = rig.serve(PROTOCOL);
    controller.wait_until("a live watch", |seen| {
        seen.watches.contains(&(BACKLOG_END, true))
    });
}

#[test]
fn failing_watches_are_dialled_with_growing_waits() {
    let mut rig = Rig::new();
    let controller = rig.serve(PROTOCOL);
    controller.fail_watches();
    rig.spawn_daemon();
    controller.wait_until("a first watch", |seen| !seen.watches.is_empty());
    std::thread::sleep(Duration::from_millis(1600));
    let watches = controller.seen(|seen| seen.watches.len());
    assert!(watches <= 4, "{watches} watches in 1.6 s");
    let stderr = rig.stderr();
    assert_eq!(
        stderr.matches("cannot read the event store").count(),
        1,
        "{stderr}"
    );
}

#[test]
fn protocol_mismatch_is_logged_once() {
    let mut rig = Rig::new();
    let newer = PROTOCOL + 1;
    let controller = rig.serve(newer);
    rig.spawn_daemon();
    controller.wait_until("three hellos", |seen| seen.hellos.len() >= 3);
    let stderr = rig.stderr();
    let lines = stderr
        .lines()
        .filter(|l| l.contains(&format!("controller speaks {newer}")))
        .count();
    assert_eq!(lines, 1, "{stderr}");
}

/// Makes `file` readable by everyone, spawns the daemon and checks that it refuses to start,
/// names `label` and the mode it wants, and keeps `token` off stderr.
fn refuses_shared_token_file(file: &str, label: &str, token: &str) {
    let mut rig = Rig::new();
    std::fs::set_permissions(rig.path(file), std::fs::Permissions::from_mode(0o644)).unwrap();
    rig.spawn_daemon();
    let status = rig.wait_exit(Duration::from_secs(5));
    assert!(status.is_some_and(|s| !s.success()), "{status:?}");
    let stderr = rig.stderr();
    assert!(stderr.contains(label), "{stderr}");
    assert!(stderr.contains("0600"), "{stderr}");
    assert!(!stderr.contains(token), "{stderr}");
}

#[test]
fn slack_token_file_must_be_owner_only() {
    refuses_shared_token_file("app.token", "slack app token file", APP_TOKEN);
}

#[test]
fn slack_bot_token_file_must_be_owner_only() {
    refuses_shared_token_file("bot.token", "slack bot token file", BOT_TOKEN);
}

#[test]
fn token_paths_starting_with_a_tilde_are_read_from_home() {
    let mut rig = Rig::new();
    std::fs::create_dir(rig.path("home")).unwrap();
    for file in ["app.token", "bot.token"] {
        std::fs::rename(rig.path(file), rig.path("home").join(file)).unwrap();
    }
    rig.write_config("~/app.token", "~/bot.token");
    rig.spawn_daemon();
    rig.slack.wait_connected();
    assert!(rig
        .slack
        .requests()
        .iter()
        .filter(|r| r.path == "/apps.connections.open")
        .all(|r| r.auth == format!("Bearer {APP_TOKEN}")));
}
