//! Tests for the host Session requests.

use super::*;
use crate::name::SessionId;
use serde_json::json;

#[test]
fn update_host_session_round_trips() {
    let wire = json!({"cmd": "update-host-session", "session": 3, "agent_session_id": "abc"});
    let req: Request = serde_json::from_value(wire.clone()).expect("parse");
    assert_eq!(
        req,
        Request::UpdateHostSession(UpdateHostSessionReq {
            session: SessionId::new(3),
            agent: None,
            agent_session_id: Some("abc".into())
        })
    );
    assert_eq!(req.cmd(), "update-host-session");
    assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    assert!(serde_json::from_value::<Request>(
        json!({"cmd": "update-host-session", "session": 3, "bogus": 1})
    )
    .is_err());
}

#[test]
fn end_host_session_round_trips() {
    let wire = json!({"cmd": "end-host-session", "session": 3});
    let req: Request = serde_json::from_value(wire.clone()).expect("parse");
    assert_eq!(
        req,
        Request::EndHostSession(EndHostSessionReq {
            session: SessionId::new(3),
        })
    );
    assert_eq!(req.cmd(), "end-host-session");
    assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    assert!(serde_json::from_value::<Request>(
        json!({"cmd": "end-host-session", "session": 3, "bogus": 1})
    )
    .is_err());
}
