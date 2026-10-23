//! The daemon's socket comes from `MOOR_FRONT_SOCKET`, and SIGTERM stops it cleanly.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
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

/// A controller-run front has a loopback of its own, so a Slack on the host's loopback, as
/// this fake is, never hears from it; the test itself reaches the fake as a control.
#[test]
fn run_by_the_controller_it_reaches_no_slack_on_the_hosts_loopback() {
    let rig = Rig::new();
    let (_controller, state) = rig.real_controller_running_the_daemon();
    let pid_file = rig.path("state").join("fronts").join("slack.pid");
    for _ in 0..500 {
        if pid_file.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pid_file.exists(), "the controller started the front");
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        rig.slack.requests().is_empty(),
        "{:?}",
        rig.slack.requests()
    );
    assert!(!state.path().join("slack.db").exists());
    let port = rig.slack.api_base().rsplit_once(':').unwrap().1.to_owned();
    let mut control = TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
    control
        .write_all(b"POST /auth.test HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .unwrap();
    let _ = control.read_to_end(&mut Vec::new());
    assert_eq!(rig.slack.requests().len(), 1, "the host reaches the fake");
}
