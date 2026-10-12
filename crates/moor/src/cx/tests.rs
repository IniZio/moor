//! Tests for `Cx`.

use super::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;
use moor_core::seam::clock::Ts;
use moor_core::Warning;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Context over fakes with state under `dir`.
fn cx(dir: &Path) -> Cx {
    Cx::fake(
        dir,
        Path::new("/w"),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new("/s")),
        Arc::new(ScriptedProc::new()),
    )
}

/// Build a warning.
fn w(code: &str) -> Warning {
    Warning {
        code: code.to_owned(),
        message: format!("msg {code}"),
    }
}

#[test]
fn warnings_drain_in_order() {
    let cx = cx(Path::new("/nonexistent"));
    cx.warn(w("a"));
    cx.warn(w("b"));
    let got = cx.take_warnings();
    assert_eq!(got.len(), 2);
    assert_eq!(got.first().map(|x| x.code.as_str()), Some("a"));
    assert!(cx.take_warnings().is_empty());
}

#[test]
fn store_opens_lazily_at_moor_db() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cx = cx(dir.path());
    assert!(!dir.path().join("moor.db").exists());
    assert!(cx.store().is_ok());
    assert!(dir.path().join("moor.db").exists());
    assert!(cx.store().is_ok());
}

#[test]
fn accessors_return_injected_values() {
    let cx = cx(Path::new("/nonexistent"));
    assert_eq!(cx.cwd(), Path::new("/w"));
    assert_eq!(cx.config().herdr_session(), "moor");
}

#[test]
fn clock_is_the_injected_one() {
    let cx = cx(Path::new("/nonexistent"));
    assert_eq!(cx.clock().now(), Ts::from_millis(1000));
    cx.clock().sleep(Duration::from_millis(250));
    assert_eq!(cx.clock().now(), Ts::from_millis(1250));
}
