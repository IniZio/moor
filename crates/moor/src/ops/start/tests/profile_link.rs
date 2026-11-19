//! A machine's link that drops while the Box fetches or installs the agent profile: the step is
//! asked again once the link is back, and a failure that outlasts the wait fails setup, so the
//! agent never starts without the profile.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::{BoxName, BoxReq};

use super::profile::{answering, set_profile, HEAD};
use super::*;
use crate::testing::driver::RELINK_ASKS;
use crate::testing::engine::FakeEngine;

/// What the profile fetch runs.
const FETCH: &str = "git fetch";
/// What the profile's plugin installs run.
const PLUGINS: &str = "echo install-the-plugins";
const MANIFEST: &str = "claude: {model: opus}\n";

/// What the machine adapter says while the controller holds no link to the machine.
fn unlinked() -> DriverError {
    DriverError::Unavailable("machine 1 is not linked".to_owned())
}

/// A world with a profile whose plugins install with [`PLUGINS`], over `driver`.
fn profiled(driver: FakeDriver) -> (World, Arc<FakeEngine>) {
    let mut w = answering(driver, None, HEAD, Some(MANIFEST));
    let engine = Arc::new(FakeEngine::new().profile_install(PLUGINS));
    w.cx = w.cx.with_engine(engine.clone());
    set_profile(&w, None);
    w.cx.take_warnings();
    (w, engine)
}

/// `driver` with the link down for the next `asks` execs that mention `needle`.
fn link_down_for(driver: FakeDriver, needle: &str, asks: usize) -> FakeDriver {
    (0..asks).fold(driver, |d, _| d.exec_fails_once_when(needle, unlinked()))
}

fn asked(w: &World, needle: &str) -> usize {
    let execs = w.driver.execs();
    execs
        .iter()
        .filter(|(_, spec, _)| spec.argv.join(" ").contains(needle))
        .count()
}

fn kinds(w: &World, id: BoxId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 200).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

/// The `line` of Box `id`'s `setup_failed` event.
fn setup_failed_line(w: &World, id: BoxId) -> String {
    let events = w.cx.store().unwrap().events(id, 200).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    let line = failed.payload.get("line").and_then(|l| l.as_str());
    line.unwrap().to_owned()
}

#[test]
fn a_link_drop_during_the_profile_fetch_waits_for_the_relink() {
    let (w, engine) = profiled(link_down_for(FakeDriver::new(), FETCH, 2));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), state::WORKING);
    let id = BoxId::new(v.boxed.id);
    assert!(!kinds(&w, id).contains(&"profile_failed".to_owned()));
    assert_eq!(asked(&w, FETCH), 3, "two asks while down, one after");
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn a_link_drop_during_the_plugin_installs_waits_for_the_relink() {
    let (w, engine) = profiled(link_down_for(FakeDriver::new(), PLUGINS, 2));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), state::WORKING);
    let id = BoxId::new(v.boxed.id);
    assert!(!kinds(&w, id).contains(&"profile_failed".to_owned()));
    assert_eq!(asked(&w, PLUGINS), 3);
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn a_profile_fetch_whose_link_stays_down_fails_setup_with_the_reason() {
    let (w, engine) = profiled(FakeDriver::new().exec_fails_when(FETCH, unlinked()));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(v.failed_step.as_deref(), Some("profile"));
    assert!(engine.launches().is_empty());
    let id = BoxId::new(v.boxed.id);
    let line = setup_failed_line(&w, id);
    assert!(line.starts_with("agent profile: "), "{line}");
    assert!(line.contains("not linked"), "{line}");
    assert_eq!(
        asked(&w, PLUGINS),
        0,
        "nothing installs from a profile never fetched"
    );
}

#[test]
fn a_failed_plugin_install_fails_setup_with_the_reason() {
    let driver = FakeDriver::new().exec_ends_when(PLUGINS, ExecOutcome::Exited { code: 2 });
    let (w, engine) = profiled(driver);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(v.failed_step.as_deref(), Some("profile"));
    assert!(engine.launches().is_empty());
    let line = setup_failed_line(&w, BoxId::new(v.boxed.id));
    assert!(line.starts_with("agent profile: "), "{line}");
    assert!(line.contains("exit 2"), "{line}");
}

#[test]
fn a_retry_once_the_link_is_back_fetches_the_profile_and_launches() {
    let (w, engine) = profiled(link_down_for(FakeDriver::new(), FETCH, RELINK_ASKS));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::SetupFailed);
    let retry = BoxReq {
        box_name: BoxName::parse_allow_bare("demo").unwrap(),
    };
    let again = crate::ops::retry::run(&w.cx, &retry).unwrap();
    assert_eq!(
        (again.boxed.lifecycle, again.boxed.session_state),
        state::WORKING
    );
    assert_eq!(again.failed_step, None);
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn a_failed_install_says_why_in_its_event_and_in_why_setup_failed() {
    let why =
        "marketplace tools exit 1: Failed to clone marketplace repository: Git clone timed out";
    let (w, _) = profiled(FakeDriver::new());
    let driver = w.driver.clone();
    w.driver.on_exec_of(PLUGINS, move || {
        driver.push_exec_stdout(format!("moor: profile: {why}\n"));
    });
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::SetupFailed);
    let id = BoxId::new(v.boxed.id);
    let events = w.cx.store().unwrap().events(id, 200).unwrap();
    let failed = events.iter().find(|e| e.kind == "profile_failed").unwrap();
    assert_eq!(failed.payload.get("detail"), Some(&serde_json::json!(why)));
    let line = setup_failed_line(&w, id);
    assert_eq!(
        line,
        format!("agent profile: was only partly installed: {why}")
    );
}
