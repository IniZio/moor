//! Tests for the `in` request's wire shape and split.

use std::path::Path;

use serde_json::json;

use crate::request::Request;

fn parse(wire: serde_json::Value) -> Request {
    serde_json::from_value(wire).unwrap()
}

#[test]
fn in_round_trips_and_splits_into_its_dir_and_request() {
    let wire = json!({"cmd": "in", "dir": "/w/b", "request": {"cmd": "env", "repo": "/r"}});
    let req = parse(wire.clone());
    assert_eq!(req.cmd(), "in");
    assert_eq!(serde_json::to_value(&req).unwrap(), wire);
    let (dir, inner) = req.split_dir().unwrap();
    assert_eq!(dir.as_deref(), Some(Path::new("/w/b")));
    assert_eq!(inner.cmd(), "env");
}

#[test]
fn a_request_without_in_names_no_dir() {
    let (dir, inner) = parse(json!({"cmd": "version"})).split_dir().unwrap();
    assert_eq!(dir, None);
    assert_eq!(inner.cmd(), "version");
}

#[test]
fn a_relative_dir_or_a_nested_in_is_refused() {
    let relative = json!({"cmd": "in", "dir": "b", "request": {"cmd": "version"}});
    assert!(parse(relative)
        .split_dir()
        .unwrap_err()
        .contains("relative"));
    let nested = json!({"cmd": "in", "dir": "/a", "request":
        {"cmd": "in", "dir": "/b", "request": {"cmd": "version"}}});
    assert!(parse(nested)
        .split_dir()
        .unwrap_err()
        .contains("another in"));
}
