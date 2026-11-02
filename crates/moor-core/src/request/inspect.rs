//! Request that looks at one Box to tell working from stuck.

use serde::{Deserialize, Serialize};

use super::BoxTarget;

/// Arguments of `inspect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectReq {
    /// The Box, in any state: by name, the newest Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// Pane lines wanted; 40 when absent, at most 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// Agent messages wanted; 5 when absent, at most 20.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::super::Request;
    use super::*;
    use crate::name::BoxName;
    use serde_json::json;

    fn target() -> BoxTarget {
        BoxName::parse_allow_bare("a").expect("name").into()
    }

    #[test]
    fn round_trips_without_options() {
        let wire = json!({"cmd": "inspect", "box": "a"});
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        assert_eq!(
            req,
            Request::Inspect(InspectReq {
                target: target(),
                lines: None,
                said: None
            })
        );
        assert_eq!(req.cmd(), "inspect");
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn round_trips_with_lines_and_said() {
        let wire = json!({"cmd": "inspect", "box": "a", "lines": 100, "said": 8});
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        assert_eq!(
            req,
            Request::Inspect(InspectReq {
                target: target(),
                lines: Some(100),
                said: Some(8)
            })
        );
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn refuses_unknown_field() {
        let wire = json!({"cmd": "inspect", "box": "a", "pane": true});
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }

    #[test]
    fn names_its_box() {
        let req = Request::Inspect(InspectReq {
            target: target(),
            lines: None,
            said: None,
        });
        assert_eq!(req.names(), vec![super::super::Named::Box(target())]);
    }
}
