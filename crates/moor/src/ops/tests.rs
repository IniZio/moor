use moor_core::Request;

use std::sync::Arc;

use super::dispatch;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

/// Context over fakes with no usable state.
fn cx() -> Cx {
    let p = std::path::Path::new("/nonexistent");
    Cx::fake(
        p,
        p,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
}

#[test]
fn version_reports_build_info() {
    let cx = cx();
    let v = dispatch(&cx, Request::Version {}).expect("version ok");
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
