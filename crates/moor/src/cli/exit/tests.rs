//! Tests for the exit-code table.

use moor_core::{ErrorKind, ErrorView, Response};
use serde_json::json;

use super::code;

/// Builds an error response of `kind` for `cmd`.
fn err(cmd: &str, kind: ErrorKind) -> Response {
    Response::err(
        cmd,
        ErrorView {
            kind,
            message: "m".into(),
            hints: vec![],
            context: vec![],
        },
    )
}

#[test]
fn classes_map_to_codes() {
    assert_eq!(code(&Response::ok("version", None)), 0);
    assert_eq!(code(&err("show", ErrorKind::Failed)), 1);
    assert_eq!(code(&err("show", ErrorKind::Internal)), 1);
    assert_eq!(code(&err("show", ErrorKind::Usage)), 2);
    assert_eq!(code(&err("show", ErrorKind::Refused)), 3);
}

#[test]
fn run_failed_and_refused_are_125() {
    assert_eq!(code(&err("run", ErrorKind::Failed)), 125);
    assert_eq!(code(&err("run", ErrorKind::Internal)), 125);
    assert_eq!(code(&err("run", ErrorKind::Refused)), 125);
}

#[test]
fn run_usage_is_125() {
    assert_eq!(code(&err("run", ErrorKind::Usage)), 125);
}

#[test]
fn run_ok_uses_command_exit() {
    assert_eq!(code(&Response::ok("run", Some(json!({"exit": 7})))), 7);
    assert_eq!(code(&Response::ok("run", Some(json!({"exit": 0})))), 0);
}

#[test]
fn run_ok_without_usable_exit_is_0() {
    assert_eq!(code(&Response::ok("run", None)), 0);
    assert_eq!(code(&Response::ok("run", Some(json!({"other": 1})))), 0);
    assert_eq!(code(&Response::ok("run", Some(json!({"exit": 300})))), 0);
    assert_eq!(code(&Response::ok("run", Some(json!({"exit": "x"})))), 0);
}

#[test]
fn non_run_ignores_exit_field() {
    assert_eq!(code(&Response::ok("show", Some(json!({"exit": 7})))), 0);
}

#[test]
fn wait_timed_out_is_124() {
    let resp = Response::ok("wait", Some(json!({"timed_out": true})));
    assert_eq!(code(&resp), 124);
}

#[test]
fn wait_settled_is_0() {
    assert_eq!(
        code(&Response::ok("wait", Some(json!({"timed_out": false})))),
        0
    );
    assert_eq!(code(&Response::ok("wait", None)), 0);
    assert_eq!(
        code(&Response::ok("show", Some(json!({"timed_out": true})))),
        0
    );
}

#[test]
fn run_interrupted_is_130() {
    let v = json!({"exit": null, "interrupted": true});
    assert_eq!(code(&Response::ok("run", Some(v))), 130);
}

#[test]
fn run_interrupted_wins_over_exit() {
    let v = json!({"exit": 0, "interrupted": true});
    assert_eq!(code(&Response::ok("run", Some(v))), 130);
    let v = json!({"exit": 7, "interrupted": true});
    assert_eq!(code(&Response::ok("run", Some(v))), 130);
}

#[test]
fn run_not_interrupted_passes_exit_through() {
    let v = json!({"exit": 7, "interrupted": false});
    assert_eq!(code(&Response::ok("run", Some(v))), 7);
}

#[test]
fn non_run_ignores_interrupted() {
    let v = json!({"interrupted": true});
    assert_eq!(code(&Response::ok("show", Some(v))), 0);
}
