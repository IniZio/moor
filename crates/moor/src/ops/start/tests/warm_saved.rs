//! What a warm start says it spared: the cold setup of the layer it forked, whatever the Box that
//! saved that layer stood on.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::StartReq;

use super::warm::{events, start, LINES};
use super::{agentless_req, repo, world, World};
use crate::cx::Cx;
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::ops::start::run;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

/// A world whose clock moves `clone_s` during the first clone only and 25 s during `two-b`.
fn timed(clone_s: u64) -> World {
    let driver = FakeDriver::new().with_checkpoints();
    let mut w = world(driver);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    w.cx = Cx::fake_with_clock(
        w.dir.path(),
        w.driver.clone(),
        w.proc.clone(),
        moor_core::config::UserConfig::default(),
        clock.clone(),
    )
    .with_forge(w.forge.clone());
    let slow = clock.clone();
    w.driver.on_exec_of("two-b", move || {
        slow.sleep(Duration::from_secs(25));
    });
    let once = AtomicBool::new(true);
    w.driver.on_exec_of("git clone", move || {
        if once.swap(false, Ordering::SeqCst) {
            clock.sleep(Duration::from_secs(clone_s));
        }
    });
    w
}

#[test]
fn saved_ms_reports_the_cold_setup_of_the_layer_forked() {
    let w = timed(40);
    start(&w, "first", "abc123", LINES);
    let again = start(&w, "again", "abc123", LINES);
    let warm = events(&w, again, "warm_started");
    assert_eq!(warm[0]["layer"], 3);
    assert_eq!(warm[0]["saved_ms"], 65_000, "{warm:?}");
}

#[test]
fn a_fork_of_the_clone_keeps_the_cold_clone_time_in_what_it_saves_above() {
    let w = timed(40);
    start(&w, "first", "abc123", "setup: [one-a]\n");
    let up = start(&w, "up", "abc123", "setup: [one-z, two-b]\n");
    assert_eq!(events(&w, up, "warm_started")[0]["layer"], 0);
    let again = start(&w, "again", "abc123", "setup: [one-z, two-b]\n");
    let warm = events(&w, again, "warm_started");
    assert_eq!(warm[0]["layer"], 2);
    assert_eq!(warm[0]["saved_ms"], 65_000, "{warm:?}");
}

#[test]
fn a_restart_keeps_the_cold_time_of_the_layer_it_stands_on() {
    let w = timed(40);
    start(&w, "first", "abc123", "setup: [one-a]\n");
    let dying = Arc::new(AtomicBool::new(true));
    let crash = dying.clone();
    w.driver.on_exec_of("two-b", move || {
        assert!(!crash.load(Ordering::SeqCst), "the controller dies here");
    });
    let yaml = "setup: [one-a, two-b]\n";
    w.forge.answer(repo(), Ok(repo_at("abc123", Some(yaml))));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("up")
    };
    assert!(catch_unwind(AssertUnwindSafe(|| drop(run(&w.cx, &req)))).is_err());
    dying.store(false, Ordering::SeqCst);
    let store = w.cx.store().unwrap();
    let name = crate::testing::box_name("up");
    let up = store.open_box_by_name(&name).unwrap().unwrap().id;
    fail_interrupted(&w.cx, store).unwrap();
    resume_setup(&w.cx, store, up).unwrap();
    let again = start(&w, "again", "abc123", yaml);
    let warm = events(&w, again, "warm_started");
    assert_eq!(warm[0]["layer"], 2);
    assert!(warm[0]["saved_ms"].as_u64().unwrap() >= 40_000, "{warm:?}");
}
