//! Each step of a start that waits on something carries its time in the Box's events.

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::driver::DriverError;
use moor_core::seam::engine::EngineError;
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::{BoxId, StartReq};
use serde_json::{json, Value};

use super::agent_install::step_events_where;
use super::{repo_req, run, script_base_yaml, world_with, World, BASE};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::forge::repo_at;
use crate::testing::proxy_host::FakeProxyHost;

/// A world over `demo` whose clock the test moves.
fn clocked(driver: FakeDriver) -> (World, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let mut w = world_with(driver, UserConfig::default());
    script_base_yaml(&w, Some("setup: [a]\n"));
    w.cx = w.cx.with_clock(clock.clone());
    (w, clock)
}

/// The events of `step` on Box 1, as kind and payload.
fn of(w: &World, step: &str) -> Vec<(String, Value)> {
    step_events_where(w, BoxId::new(1), |s| s == step)
}

fn pair(step: &str, ms: u64, ok: bool) -> Vec<(String, Value)> {
    vec![
        ("step_started".to_owned(), json!({"step": step})),
        (
            "step_finished".to_owned(),
            json!({"step": step, "ms": ms, "ok": ok}),
        ),
    ]
}

#[test]
fn the_machine_acquire_records_the_time_the_driver_took() {
    let driver = FakeDriver::new();
    let (w, clock) = clocked(driver);
    w.driver
        .notes_while_acquiring(&[], move || clock.sleep(Duration::from_secs(58)));
    run(&w.cx, &repo_req("demo")).unwrap();
    assert_eq!(of(&w, "acquire"), pair("acquire", 58_000, true));
}

#[test]
fn a_failed_acquire_records_ok_false() {
    let driver = FakeDriver::new().fail_acquire(DriverError::Unavailable("no capacity".into()));
    let (w, _) = clocked(driver);
    let _ = run(&w.cx, &repo_req("demo"));
    assert_eq!(of(&w, "acquire"), pair("acquire", 0, false));
}

#[test]
fn the_proxy_start_records_a_step_and_a_failed_one_ends_not_ok() {
    let (w, _) = clocked(FakeDriver::new());
    run(&w.cx, &repo_req("demo")).unwrap();
    assert_eq!(of(&w, "proxy"), pair("proxy", 0, true));

    let (mut w, _) = clocked(FakeDriver::new());
    let host = FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("boom".into()));
    w.cx = w.cx.with_proxy_host(Arc::new(host));
    let _ = run(&w.cx, &repo_req("demo"));
    assert_eq!(of(&w, "proxy"), pair("proxy", 0, false));
}

#[test]
fn the_other_repos_clone_as_one_step_with_the_batch_time() {
    let (w, clock) = clocked(FakeDriver::new());
    let lib = ForgeRepo::new("o", "lib");
    w.forge.answer(lib.clone(), Ok(repo_at(BASE, None)));
    w.driver
        .on_exec(move || clock.sleep(Duration::from_secs(5)));
    let req = StartReq {
        also: Some(vec![lib]),
        ..repo_req("demo")
    };
    run(&w.cx, &req).unwrap();
    assert_eq!(of(&w, "clone_others"), pair("clone_others", 5_000, true));
}

#[test]
fn a_start_with_no_other_repos_records_no_clone_others_step() {
    let (w, _) = clocked(FakeDriver::new());
    run(&w.cx, &repo_req("demo")).unwrap();
    assert!(of(&w, "clone_others").is_empty());
}

#[test]
fn the_agent_launch_records_a_step_and_a_failed_launch_ends_not_ok() {
    let (w, _) = clocked(FakeDriver::new());
    run(&w.cx, &repo_req("demo")).unwrap();
    assert_eq!(of(&w, "launch"), pair("launch", 0, true));

    let (mut w, _) = clocked(FakeDriver::new());
    let engine = FakeEngine::new().launch_result(Err(EngineError::Failed {
        step: "launch",
        message: "no pane".into(),
    }));
    w.cx = w.cx.with_engine(Arc::new(engine));
    run(&w.cx, &repo_req("demo")).unwrap();
    assert_eq!(of(&w, "launch"), pair("launch", 0, false));
}
