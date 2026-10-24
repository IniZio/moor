//! Tests for `Cx`.

use super::Cx;
use crate::env::Env;
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::BoxName;
use moor_core::Warning;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Context over fakes with state under `dir`.
fn cx(dir: &Path) -> Cx {
    Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
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
    let cx = Cx::fake_bare(
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
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

#[test]
fn every_context_uses_the_store_its_env_shares() {
    let tmp = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    let shared = Store::open(&tmp.path().join("shared").join(STORE_FILE), clock).unwrap();
    let own = tmp.path().join("own");
    let env = Env::default().sharing(shared.clone());
    let new_cx = || Cx {
        env: env.clone(),
        ..Cx::fake_bare(
            &own,
            Arc::new(FakeDriver::new()),
            Arc::new(FakeSurface::new()),
            Arc::new(ScriptedProc::new()),
        )
    };
    let name = BoxName::parse_allow_bare("a").unwrap();
    let made = new_cx()
        .store()
        .unwrap()
        .apply(Decision::insert(NewBox { name, repo: None }))
        .unwrap();
    assert!(new_cx()
        .store()
        .unwrap()
        .box_row(made.box_id)
        .unwrap()
        .is_some());
    assert!(shared.box_row(made.box_id).unwrap().is_some());
    assert!(
        !own.join(STORE_FILE).exists(),
        "a context opened its own store"
    );
}
