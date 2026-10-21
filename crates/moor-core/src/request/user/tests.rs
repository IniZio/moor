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
