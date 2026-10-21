//! Requests that list and read a box's versioned artifacts.

use serde::{Deserialize, Serialize};

use super::BoxTarget;

/// Arguments of `artifacts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactsReq {
    /// The Box, in any state: by name, the newest Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// List every version; only the latest of each artifact when false.
    #[serde(default)]
    pub all: bool,
}

/// Arguments of `artifact-read`; the bytes arrive as raw bytes frames before the response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReadReq {
    /// The Box, in any state: by name, the newest Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// Artifact name.
    pub name: String,
    /// Version to read; the latest when absent.
    #[serde(default)]
    pub version: Option<u32>,
    /// Byte offset to start at.
    #[serde(default)]
    pub offset: u64,
    /// Bytes wanted; fewer arrive when the file ends first.
    pub len: u64,
}

#[cfg(test)]
mod tests {
    use super::super::Request;
    use super::*;
    use crate::name::BoxName;
    use serde_json::json;

    fn box_name() -> BoxName {
        BoxName::parse("repo/alpha").expect("box name")
    }

    #[test]
    fn artifacts_round_trips() {
        let wire = json!({"cmd": "artifacts", "box": "repo/alpha", "all": true});
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        assert_eq!(
            req,
            Request::Artifacts(ArtifactsReq {
                target: box_name().into(),
                all: true
            })
        );
        assert_eq!(req.cmd(), "artifacts");
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn artifacts_all_defaults_false() {
        let req: Request = serde_json::from_value(json!({"cmd": "artifacts", "box": "repo/alpha"}))
            .expect("parse");
        assert_eq!(
            req,
            Request::Artifacts(ArtifactsReq {
                target: box_name().into(),
                all: false
            })
        );
    }

    #[test]
    fn artifact_read_round_trips() {
        let wire = json!({
            "cmd": "artifact-read", "box": "repo/alpha", "name": "a.txt",
            "version": 2, "offset": 10, "len": 20
        });
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        let want = ArtifactReadReq {
            target: box_name().into(),
            name: "a.txt".into(),
            version: Some(2),
            offset: 10,
            len: 20,
        };
        assert_eq!(req, Request::ArtifactRead(want));
        assert_eq!(req.cmd(), "artifact-read");
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn artifact_read_defaults_latest_from_start() {
        let wire = json!({"cmd": "artifact-read", "box": "repo/alpha", "name": "a", "len": 5});
        let req: Request = serde_json::from_value(wire).expect("parse");
        let want = ArtifactReadReq {
            target: box_name().into(),
            name: "a".into(),
            version: None,
            offset: 0,
            len: 5,
        };
        assert_eq!(req, Request::ArtifactRead(want));
    }

    #[test]
    fn artifact_read_needs_a_len() {
        let wire = json!({"cmd": "artifact-read", "box": "repo/alpha", "name": "a"});
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }

    #[test]
    fn unknown_field_is_refused() {
        let wire = json!({"cmd": "artifacts", "box": "repo/alpha", "x": 1});
        assert!(serde_json::from_value::<Request>(wire).is_err());
        let wire =
            json!({"cmd": "artifact-read", "box": "repo/alpha", "name": "a", "len": 1, "x": 1});
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }
}
