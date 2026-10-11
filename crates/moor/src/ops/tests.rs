use moor_core::{CloseReq, ErrorKind, Request};

use super::dispatch;
use crate::cx::Cx;

#[test]
fn version_reports_build_info() {
    let cx = Cx::new(None);
    let v = dispatch(&cx, Request::Version).expect("version ok");
    assert_eq!(
        v.get("version").and_then(serde_json::Value::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(v.get("schema").and_then(serde_json::Value::as_u64), Some(1));
    assert_eq!(
        v.get("commit").and_then(serde_json::Value::as_str),
        Some(env!("MOOR_COMMIT"))
    );
}

#[test]
fn other_ops_are_not_implemented() {
    let cx = Cx::new(None);
    let req = Request::Close(CloseReq {
        task: "t".to_owned(),
        force: false,
    });
    let err = dispatch(&cx, req).expect_err("not implemented");
    assert_eq!(err.kind, ErrorKind::Internal);
    assert!(err.error.to_string().contains("`moor close`"));
}
