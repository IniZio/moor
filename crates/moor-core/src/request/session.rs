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
    fn unknown_field_is_refused() {
        let wire = json!({"cmd": "show-session", "session": 5, "x": 1});
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }
}
