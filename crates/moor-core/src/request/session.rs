//! Request that reads one Session.

use serde::{Deserialize, Serialize};

use crate::name::SessionId;

/// Arguments of `show-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowSessionReq {
    /// Session id; a host Session has no Box.
    pub session: SessionId,
}

/// Arguments of `sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionsReq {
    /// Only the direct children of this Session; every live Session when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
}

/// Arguments of `end-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndSessionReq {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: String,
}

/// Arguments of `open-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSessionReq {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: String,
    /// Text the agent receives as its first prompt.
    pub seed: String,
    /// Parent Session of the Session this request opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
}

#[cfg(test)]
mod tests {
    use super::super::Request;
    use super::*;
    use serde_json::json;

    #[test]
    fn show_session_round_trips() {
        let wire = json!({"cmd": "show-session", "session": 5});
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        assert_eq!(
            req,
            Request::ShowSession(ShowSessionReq {
                session: SessionId::new(5)
            })
        );
        assert_eq!(req.cmd(), "show-session");
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn sessions_round_trips_with_and_without_parent() {
        let bare = json!({"cmd": "sessions"});
        let req: Request = serde_json::from_value(bare.clone()).expect("parse");
        assert_eq!(req, Request::Sessions(SessionsReq { parent: None }));
        assert_eq!(req.cmd(), "sessions");
        assert_eq!(serde_json::to_value(&req).expect("serialize"), bare);
        let wire = json!({"cmd": "sessions", "parent": 3});
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        assert_eq!(
            req,
            Request::Sessions(SessionsReq {
                parent: Some(SessionId::new(3))
            })
        );
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn sessions_refuses_unknown_field() {
        let wire = json!({"cmd": "sessions", "x": 1});
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }

    #[test]
    fn unknown_field_is_refused() {
        let wire = json!({"cmd": "show-session", "session": 5, "x": 1});
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }
}
