//! Tests for the user requests' wire shape.

use serde_json::json;

use super::{AsReq, UserCmd};
use crate::request::Request;
use crate::user::Identity;

#[test]
fn whoami_round_trips_with_and_without_an_identity() {
    for wire in [
        json!({"cmd": "whoami"}),
        json!({"cmd": "whoami", "identity": "slack:T1:U1"}),
    ] {
        let req: Request = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(req.cmd(), "whoami");
        assert_eq!(serde_json::to_value(&req).unwrap(), wire);
    }
}

#[test]
fn a_malformed_identity_or_unknown_field_is_refused() {
    for wire in [
        json!({"cmd": "whoami", "identity": "nobody"}),
        json!({"cmd": "whoami", "x": 1}),
    ] {
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }
}

#[test]
fn into_user_splits_out_the_asserted_identity() {
    let id = Identity::parse("web:alice").unwrap();
    let req = Request::Whoami(AsReq {
        identity: Some(id.clone()),
    });
    assert_eq!(req.into_user(), Ok((Some(id), UserCmd::Whoami)));
    let other = Request::Version {};
    assert_eq!(other.clone().into_user(), Err(Box::new(other)));
}

const CANARY: &str = "sk-ant-canary-0d9f";

fn secret_set() -> Request {
    serde_json::from_value(json!({
        "cmd": "secret-set", "identity": "slack:T1:U1", "name": "claude", "value": CANARY,
    }))
    .unwrap()
}

#[test]
fn every_user_request_round_trips_and_splits() {
    for (wire, cmd) in [
        (json!({"cmd": "enroll"}), "enroll"),
        (
            json!({"cmd": "secret-set", "name": "claude", "value": "v"}),
            "secret-set",
        ),
        (
            json!({"cmd": "secret-delete", "name": "claude"}),
            "secret-delete",
        ),
        (
            json!({"cmd": "secret-names", "identity": "web:a"}),
            "secret-names",
        ),
        (json!({"cmd": "link-code"}), "link-code"),
        (json!({"cmd": "link-redeem", "code": "c0de"}), "link-redeem"),
    ] {
        let req: Request = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(req.cmd(), cmd);
        assert_eq!(serde_json::to_value(&req).unwrap(), wire);
        assert_eq!(req.into_user().unwrap().1.cmd(), cmd);
    }
}

#[test]
fn a_bad_secret_name_is_refused() {
    let wire = json!({"cmd": "secret-delete", "name": "Not/A/Name"});
    assert!(serde_json::from_value::<Request>(wire).is_err());
}

#[test]
fn debug_of_a_secret_set_never_shows_the_value() {
    let req = secret_set();
    let line = format!("moor controller: request {req:?}");
    assert!(!line.contains(CANARY), "{line}");
    assert!(line.contains("<redacted>"), "{line}");
    let (_, cmd) = req.into_user().unwrap();
    for shown in [format!("{cmd:?}"), format!("{cmd:#?}")] {
        assert!(!shown.contains(CANARY), "{shown}");
    }
}

#[test]
fn the_value_crosses_the_wire_only_inside_secret_set() {
    let req = secret_set();
    let wire = serde_json::to_string(&req).unwrap();
    assert!(wire.contains(CANARY), "{wire}");
    let back: Request = serde_json::from_str(&wire).unwrap();
    let Ok((_, UserCmd::SecretSet { value, .. })) = back.into_user() else {
        unreachable!("secret-set splits to SecretSet");
    };
    assert_eq!(value.secret().expose(), CANARY.as_bytes());
}

#[test]
fn as_carries_the_identity_a_front_asserts_for_another_request() {
    let wire = json!({
        "cmd": "as",
        "identity": "slack:T1:U1",
        "request": {"cmd": "steer", "box": "b", "text": "hi"},
    });
    let req: Request = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(req.cmd(), "as");
    assert_eq!(serde_json::to_value(&req).unwrap(), wire);
    let (who, inner) = req.split_identity().unwrap();
    assert_eq!(who, Some(Identity::parse("slack:T1:U1").unwrap()));
    assert_eq!(inner.cmd(), "steer");
}

#[test]
fn a_request_outside_as_asserts_its_own_identity_field() {
    let own: Request =
        serde_json::from_value(json!({"cmd": "whoami", "identity": "web:a"})).unwrap();
    assert_eq!(
        own.split_identity().unwrap().0,
        Some(Identity::parse("web:a").unwrap())
    );
    let none: Request = serde_json::from_value(json!({"cmd": "show"})).unwrap();
    assert_eq!(none.split_identity().unwrap().0, None);
}

#[test]
fn as_refuses_a_second_identity_or_another_as_inside() {
    let same = json!({"cmd": "as", "identity": "web:a", "request": {"cmd": "whoami", "identity": "web:a"}});
    let same: Request = serde_json::from_value(same).unwrap();
    assert_eq!(
        same.split_identity().unwrap().0,
        Some(Identity::parse("web:a").unwrap())
    );
    let other = json!({"cmd": "as", "identity": "web:a", "request": {"cmd": "whoami", "identity": "web:b"}});
    let other: Request = serde_json::from_value(other).unwrap();
    let err = other.split_identity().unwrap_err();
    assert!(err.contains("asserts web:b inside as web:a"), "{err}");
    let nested = json!({"cmd": "as", "identity": "web:a",
        "request": {"cmd": "as", "identity": "web:a", "request": {"cmd": "show"}}});
    let nested: Request = serde_json::from_value(nested).unwrap();
    let err = nested.split_identity().unwrap_err();
    assert!(err.contains("cannot wrap another as"), "{err}");
}

#[test]
fn machine_requests_round_trip() {
    for wire in [
        json!({"cmd": "machine-add", "name": "mac", "ssh": "u@h", "labels": ["macos"]}),
        json!({"cmd": "machine-add", "name": "mac", "ssh": "u@h", "identity": "slack:T1:U1"}),
        json!({"cmd": "machine-remove", "name": "mac"}),
        json!({"cmd": "machine-list"}),
    ] {
        let req: Request = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&req).unwrap(), wire);
        let (_, cmd) = req.into_user().unwrap();
        assert!(cmd.cmd().starts_with("machine-"));
    }
}
