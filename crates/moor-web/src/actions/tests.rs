//! Tests for the action body to request translation.
#![allow(clippy::unwrap_used, reason = "tests assert on known-good fixtures")]

use moor_core::request::{CloseReq, EndSessionReq, Request, SteerReq};
use moor_core::BoxName;
use serde_json::{json, Value};

use super::steer::{build, PATHS};

fn parsed(name: &str) -> BoxName {
    BoxName::parse_allow_bare(name).unwrap()
}

fn built(path: &str, body: &Value) -> Result<Request, String> {
    build(path, body.to_string().as_bytes())
}

fn steer(name: &str, text: Option<&str>, menu: Option<u32>) -> Request {
    Request::Steer(SteerReq {
        box_name: parsed(name),
        text: text.map(Into::into),
        menu,
    })
}

#[test]
fn steer_sends_text() {
    let req = built("/steer", &json!({"box": "t", "text": "hello"}));
    assert_eq!(req, Ok(steer("t", Some("hello"), None)));
}

#[test]
fn answer_by_option_sends_the_menu_number() {
    let req = built("/answer", &json!({"box": "t", "option": 2}));
    assert_eq!(req, Ok(steer("t", None, Some(2))));
}

#[test]
fn answer_by_text_sends_text() {
    let req = built("/answer", &json!({"box": "t", "text": "yes"}));
    assert_eq!(req, Ok(steer("t", Some("yes"), None)));
}

#[test]
fn close_sends_a_plain_close() {
    let req = built("/close", &json!({"box": "t"}));
    assert_eq!(
        req,
        Ok(Request::Close(CloseReq {
            box_name: parsed("t"),
            force: false,
            trust_moor_yaml: false,
        }))
    );
}

#[test]
fn end_session_builds_end_session_with_the_original_name() {
    let req = built("/end-session", &json!({"box": "app/t"}));
    assert_eq!(
        req,
        Ok(Request::EndSession(EndSessionReq {
            box_name: "app/t".into()
        }))
    );
}

#[test]
fn end_session_rejects_a_bad_name() {
    assert!(built("/end-session", &json!({"box": "Not A Name"})).is_err());
}

#[test]
fn malformed_bodies_are_errors() {
    let bad = [
        ("/close", "{not json".to_string()),
        ("/close", json!({"box": "t", "extra": 1}).to_string()),
        ("/close", json!({"box": "Not A Name"}).to_string()),
        ("/steer", json!({"box": "t"}).to_string()),
        (
            "/steer",
            json!({"box": "Not A Name", "text": "x"}).to_string(),
        ),
        (
            "/answer",
            json!({"box": "t", "option": 1, "text": "x"}).to_string(),
        ),
        ("/answer", json!({"box": "t"}).to_string()),
        ("/end-session", "{not json".to_string()),
    ];
    for (path, b) in &bad {
        assert!(build(path, b.as_bytes()).is_err(), "{path} {b}");
    }
}

#[test]
fn bad_json_says_so() {
    let err = build("/close", b"{not json").unwrap_err();
    assert!(err.starts_with("bad json: "), "{err}");
}

#[test]
fn option_zero_is_rejected() {
    let err = built("/answer", &json!({"box": "t", "option": 0})).unwrap_err();
    assert_eq!(err, "option is 1-based");
}

#[test]
fn both_or_neither_of_option_and_text_is_rejected() {
    let both = built("/answer", &json!({"box": "t", "option": 1, "text": "x"}));
    let neither = built("/answer", &json!({"box": "t"}));
    assert_eq!(both, Err("give exactly one of option or text".into()));
    assert_eq!(neither, Err("give exactly one of option or text".into()));
}

#[test]
fn unknown_field_is_rejected() {
    let err = built("/steer", &json!({"box": "t", "text": "x", "force": true})).unwrap_err();
    assert!(err.starts_with("bad json: "), "{err}");
}

#[test]
fn unknown_path_is_rejected() {
    let err = built("/start", &json!({"box": "t"})).unwrap_err();
    assert_eq!(err, "unknown action");
}

#[test]
fn every_listed_path_builds_with_a_valid_body() {
    for path in PATHS {
        let body = match *path {
            "/steer" => json!({"box": "t", "text": "x"}),
            "/answer" => json!({"box": "t", "option": 1}),
            _ => json!({"box": "t"}),
        };
        assert!(built(path, &body).is_ok(), "{path}");
    }
}
