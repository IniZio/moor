//! The daemon's socket comes from `MOOR_FRONT_SOCKET`, and SIGTERM stops it cleanly.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::time::Duration;

use moor_core::controller::PROTOCOL;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::support::controller::Scripted;
use super::support::{started_line, Rig, BOX_NAME, QUIET, TS, USER};

/// Long enough for a loaded host to start the daemon and stop it.
const LIMIT: Duration = Duration::from_secs(10);

#[test]
fn front_socket_is_dialled_when_no_flag_is_given() {
    let mut rig = Rig::new();
    let controller = rig.serve(PROTOCOL);
    let socket = rig.socket();
    rig.spawn_daemon_without_socket(|d| {
        d.env("MOOR_FRONT_SOCKET", socket);
    });
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    controller.wait_until("a hello", |seen| !seen.hellos.is_empty());
}

#[test]
fn the_flag_wins_over_front_socket() {
    let mut rig = Rig::new();
    let controller = rig.serve(PROTOCOL);
    let socket = rig.socket();
    rig.spawn_daemon_without_socket(|d| {
        d.arg("--socket")
            .arg(socket)
            .env("MOOR_FRONT_SOCKET", "/nonexistent/front.sock");
    });
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    controller.wait_until("a hello", |seen| !seen.hellos.is_empty());
}

#[test]
fn no_socket_at_all_is_refused_with_a_message() {
    let mut rig = Rig::new();
    rig.spawn_daemon_without_socket(|_| {});
    let status = rig.wait_exit(LIMIT).expect("the daemon exits");
    assert!(!status.success());
    assert!(
        rig.stderr().contains("MOOR_FRONT_SOCKET"),
        "{}",
        rig.stderr()
    );
}

#[test]
fn sigterm_stops_an_idle_daemon_with_exit_0() {
    let (mut rig, _controller) = Rig::start();
    rig.terminate_daemon();
    let status = rig.wait_exit(LIMIT).expect("the daemon exits");
    assert!(status.success(), "{status:?}\n{}", rig.stderr());
}

#[test]
fn an_event_arriving_after_sigterm_is_never_acked() {
    let (mut rig, _controller) = Rig::start();
    rig.terminate_daemon();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    let status = rig.wait_exit(LIMIT).expect("the daemon exits");
    assert!(status.success(), "{status:?}\n{}", rig.stderr());
    std::thread::sleep(QUIET);
    assert!(rig.slack.acks().is_empty(), "{:?}", rig.slack.acks());
}

#[test]
fn sigterm_lets_the_request_in_flight_finish_before_exit_0() {
    let (mut rig, controller, bin) = Rig::start_boxes();
    Rig::tool_calls(
        &bin,
        None,
        &[("start_box", json!({"goal": "fix the thing"}))],
    );
    let value = json!({"box": {"id": 7, "name": BOX_NAME, "branch": format!("moor/{BOX_NAME}")}});
    let reply = Response::ok("start", Some(value));
    controller.script("start", Scripted::Slow(Duration::from_secs(2), reply));
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    controller.wait_until("the slow start", |seen| {
        seen.requests.iter().any(|r| matches!(r, Request::Start(_)))
    });
    rig.terminate_daemon();
    let status = rig.wait_exit(LIMIT).expect("the daemon exits");
    assert!(status.success(), "{status:?}\n{}", rig.stderr());
    rig.wait_post(&started_line(BOX_NAME));
}

#[test]
fn run_by_the_controller_it_connects_and_keeps_slack_db_in_its_own_state_dir() {
    let rig = Rig::new();
    let _controller = rig.real_controller_running_the_daemon();
    rig.slack.wait_connected();
    assert!(rig.path("state-slack").join("slack.db").exists());
}
