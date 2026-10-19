//! Requests that list and read a task's versioned artifacts.

use serde::{Deserialize, Serialize};

use crate::name::BoxName;

/// Largest `len` an artifact read returns; the moor-web client frame limit is 1 MiB.
pub const ARTIFACT_CHUNK: u64 = 512 * 1024;

/// Arguments of `artifacts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactsReq {
    /// Box name.
    pub task: BoxName,
    /// List every version; only the latest of each artifact when false.
    #[serde(default)]
    pub all: bool,
}

/// Arguments of `artifact-read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReadReq {
    /// Box name.
    pub task: BoxName,
    /// Artifact name.
    pub name: String,
    /// Version to read; the latest when absent.
    #[serde(default)]
    pub version: Option<u32>,
    /// Byte offset to start at.
    #[serde(default)]
    pub offset: u64,
    /// Bytes wanted; at most `ARTIFACT_CHUNK` are returned.
    pub len: u64,
}

#[cfg(test)]
mod tests {
    use super::super::Request;
    use super::*;
    use serde_json::json;

    fn task() -> BoxName {
        BoxName::parse("repo/alpha").expect("box name")
    }

    #[test]
    fn artifacts_round_trips() {
        let wire = json!({"cmd": "artifacts", "task": "repo/alpha", "all": true});
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        assert_eq!(
            req,
            Request::Artifacts(ArtifactsReq {
                task: task(),
                all: true
            })
        );
        assert_eq!(req.cmd(), "artifacts");
        assert_eq!(serde_json::to_value(&req).expect("serialize"), wire);
    }

    #[test]
    fn artifacts_all_defaults_false() {
        let req: Request =
            serde_json::from_value(json!({"cmd": "artifacts", "task": "repo/alpha"}))
                .expect("parse");
        assert_eq!(
            req,
            Request::Artifacts(ArtifactsReq {
                task: task(),
                all: false
            })
        );
    }

    #[test]
    fn artifact_read_round_trips() {
        let wire = json!({
            "cmd": "artifact-read", "task": "repo/alpha", "name": "a.txt",
            "version": 2, "offset": 10, "len": 20
        });
        let req: Request = serde_json::from_value(wire.clone()).expect("parse");
        let want = ArtifactReadReq {
            task: task(),
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
        let wire = json!({"cmd": "artifact-read", "task": "repo/alpha", "name": "a", "len": 5});
        let req: Request = serde_json::from_value(wire).expect("parse");
        let want = ArtifactReadReq {
            task: task(),
            name: "a".into(),
            version: None,
            offset: 0,
            len: 5,
        };
        assert_eq!(req, Request::ArtifactRead(want));
    }

    #[test]
    fn unknown_field_is_refused() {
        let wire = json!({"cmd": "artifacts", "task": "repo/alpha", "x": 1});
        assert!(serde_json::from_value::<Request>(wire).is_err());
        let wire =
            json!({"cmd": "artifact-read", "task": "repo/alpha", "name": "a", "len": 1, "x": 1});
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }
}
