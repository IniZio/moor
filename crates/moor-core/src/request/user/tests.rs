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
