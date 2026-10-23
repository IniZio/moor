//! The daemon's socket comes from `MOOR_FRONT_SOCKET`, and SIGTERM stops it cleanly.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a failed setup step should abort the test"
)]

mod support;

use std::io::{Read, Write};
use std::time::Duration;

use support::Stack;

/// Long enough for a loaded host to stop the daemon.
const LIMIT: Duration = Duration::from_secs(15);

#[test]
fn front_socket_is_dialled_when_no_flag_is_given() {
    let mut stack = Stack::start();
    stack.restart_web(true);
    let (status, body) = stack.get("/tree");
    assert_eq!(status, 200, "{body}\n{}", stack.diagnostics());
}

#[test]
fn the_flag_wins_over_front_socket() {
    let mut stack = Stack::start();
    stack.restart_web_with_front_socket("/nonexistent/front.sock");
    let (status, body) = stack.get("/tree");
    assert_eq!(status, 200, "{body}\n{}", stack.diagnostics());
}

#[test]
fn sigterm_stops_an_idle_daemon_with_exit_0() {
    let mut stack = Stack::start();
    stack.terminate_web();
    let status = stack.wait_web_exit(LIMIT).expect("the daemon exits");
    assert!(status.success(), "{status:?}\n{}", stack.diagnostics());
}

#[test]
fn sigterm_lets_a_request_in_flight_finish_before_exit_0() {
    let mut stack = Stack::start();
    let mut conn = stack.connect_web();
    write!(
        conn,
        "GET /tree HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\nConnection: close\r\n",
        stack.token()
    )
    .unwrap();
    conn.flush().unwrap();
    stack.terminate_web();
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        stack.wait_web_exit(Duration::ZERO).is_none(),
        "exited early"
    );
    conn.write_all(b"\r\n").unwrap();
    let mut out = String::new();
    conn.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 200 OK"), "got {out:?}");
    let status = stack.wait_web_exit(LIMIT).expect("the daemon exits");
    assert!(status.success(), "{status:?}\n{}", stack.diagnostics());
}

#[test]
fn run_by_the_controller_it_reaches_its_socket_and_keeps_its_db_in_its_own_state_dir() {
    let stack = Stack::start_as_front();
    let (status, body) = stack.get("/tree");
    let answered = status == 200 || body.contains("the web front socket must assert an identity");
    assert!(answered, "{status} {body}\n{}", stack.diagnostics());
    assert!(
        stack.path("web").join("web.db").exists(),
        "{}",
        stack.diagnostics()
    );
}
