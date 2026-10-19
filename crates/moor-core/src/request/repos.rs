//! Request that lists the controller's known repos.

use serde::{Deserialize, Serialize};

/// Arguments of `repos`; none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReposReq {}

#[cfg(test)]
mod tests {
    use super::super::Request;
    use super::*;
    use serde_json::json;

    #[test]
    fn repos_round_trips() {
        let wire = json!({"cmd": "repos"});
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        assert_eq!(req, Request::Repos(ReposReq {}));
        assert_eq!(req.cmd(), "repos");
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn unknown_field_is_refused() {
        let wire = json!({"cmd": "repos", "x": 1});
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }
}
