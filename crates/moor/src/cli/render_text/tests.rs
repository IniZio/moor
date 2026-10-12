//! Tests for text rendering.

use moor_core::{ErrorKind, ErrorView, Response, Warning};
use serde_json::json;

use super::render;

/// Builds a warning with `msg`.
fn warn(msg: &str) -> Warning {
    Warning {
        code: "w".into(),
        message: msg.into(),
    }
}

#[test]
fn version_line_is_exact() {
    let hash = "a".repeat(40);
    let v = json!({"version": "0.2.0", "commit": hash, "schema": 1});
    let (out, err) = render(&Response::ok("version", Some(v)), false);
    assert_eq!(out, format!("moor 0.2.0 {hash} schema=1\n"));
    assert_eq!(err, "");
}

#[test]
fn odd_version_value_falls_back_to_json() {
    let (out, _) = render(&Response::ok("version", Some(json!({"version": 3}))), false);
    assert!(out.contains("\"version\": 3"));
    assert!(out.ends_with('\n'));
}

#[test]
fn other_value_is_pretty_json_and_none_is_empty() {
    let (out, _) = render(&Response::ok("show", Some(json!({"a": 1}))), false);
    assert_eq!(out, "{\n  \"a\": 1\n}\n");
    assert_eq!(
        render(&Response::ok("show", None), false),
        (String::new(), String::new())
    );
}

#[test]
fn error_prints_message_and_hints() {
    let e = ErrorView {
        kind: ErrorKind::Usage,
        message: "bad thing".into(),
        hints: vec!["try x".into(), "or y".into()],
        context: vec![],
    };
    let (out, err) = render(&Response::err("show", e), false);
    assert_eq!(out, "");
    assert_eq!(err, "moor: bad thing\nhint: try x\nhint: or y\n");
}

#[test]
fn warnings_print_unless_quiet() {
    let resp = Response::ok("show", None).with_warnings(vec![warn("careful")]);
    assert_eq!(render(&resp, false).1, "moor: warning: careful\n");
    assert_eq!(render(&resp, true).1, "");
}
