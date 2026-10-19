//! Requests that bind a conversation key to a Session.

use serde::{Deserialize, Serialize};

use crate::name::SessionId;

/// Arguments of `bind-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindSessionReq {
    /// Front the key belongs to.
    pub front: String,
    /// Conversation key within that front.
    pub key: String,
    /// Session id; a host Session has no Box.
    pub session: SessionId,
    /// Who owns the binding.
    pub owner: String,
}

/// Arguments of `session-binding`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBindingReq {
    /// Front the key belongs to.
    pub front: String,
    /// Conversation key within that front.
    pub key: String,
}

#[cfg(test)]
mod tests {
    use super::super::Request;
    use super::*;
    use serde_json::json;

    #[test]
    fn bind_session_round_trips() {
        let wire =
            json!({"cmd": "bind-session", "front": "f", "key": "k", "session": 4, "owner": "o"});
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        assert_eq!(
            req,
            Request::BindSession(BindSessionReq {
                front: "f".into(),
                key: "k".into(),
                session: SessionId::new(4),
                owner: "o".into()
            })
        );
        assert_eq!(req.cmd(), "bind-session");
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn session_binding_round_trips() {
        let wire = json!({"cmd": "session-binding", "front": "f", "key": "k"});
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        assert_eq!(
            req,
            Request::SessionBinding(SessionBindingReq {
                front: "f".into(),
                key: "k".into()
            })
        );
        assert_eq!(req.cmd(), "session-binding");
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn unknown_fields_are_refused() {
        let bind = json!({"cmd": "bind-session", "front": "f", "key": "k", "session": 4, "owner": "o", "x": 1});
        assert!(serde_json::from_value::<Request>(bind).is_err());
        let look = json!({"cmd": "session-binding", "front": "f", "key": "k", "x": 1});
        assert!(serde_json::from_value::<Request>(look).is_err());
    }
}
