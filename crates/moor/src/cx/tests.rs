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
fn warn_once_keeps_first_warning_per_code() {
    let cx = cx(Path::new("/nonexistent"));
    cx.warn_once(w("a"));
    cx.warn_once(w("a"));
    cx.warn_once(w("b"));
    let got = cx.take_warnings();
    assert_eq!(got.len(), 2);
    assert_eq!(got.get(1).map(|x| x.code.as_str()), Some("b"));
}

#[test]
fn store_opens_lazily_at_moor_db() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cx = Cx::fake_bare(
        dir.path(),
        Path::new("/w"),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new("/s")),
        Arc::new(ScriptedProc::new()),
    );
    assert!(!dir.path().join("moor.db").exists());
    assert!(cx.store().is_ok());
    assert!(dir.path().join("moor.db").exists());
    assert!(cx.store().is_ok());
}

#[test]
fn accessors_return_injected_values() {
    let cx = cx(Path::new("/nonexistent"));
    assert_eq!(cx.dir(), Some(Path::new("/w")));
    assert_eq!(cx.config().herdr_session(), "moor");
}

#[test]
fn clock_is_the_injected_one() {
    let cx = cx(Path::new("/nonexistent"));
    assert_eq!(cx.clock().now(), Ts::from_millis(1000));
    cx.clock().sleep(Duration::from_millis(250));
    assert_eq!(cx.clock().now(), Ts::from_millis(1250));
}

#[test]
fn the_fake_context_returns_the_scripted_forge() {
    use crate::testing::forge::FakeForge;
    use moor_core::seam::forge::ForgeRepo;
    let forge = FakeForge::new().with_repo("o", ForgeRepo::new("a", "b"));
    let cx = cx(Path::new("/s")).with_forge(Arc::new(forge));
    assert_eq!(cx.forge().repo_of("o"), Some(ForgeRepo::new("a", "b")));
}
