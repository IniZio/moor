//! How far a chain may go before it starts over, and a start that asks for no chain at all.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::layers::LayerKey;
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::driver::{Driver as _, ExecOutcome};
use moor_core::{BoxId, BoxPath, BoxReq, StartReq};

use super::warm::{events, lines_on, row, start};
use super::{agentless_req, repo, world, World};
use crate::cx::Cx;
use crate::ops::start::run;
use crate::store::checkpoints::{Lineage, Place};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

const LINES: &str = "setup: [one-a]\n";

/// A world with checkpoints and a clock the test moves.
fn timed(driver: FakeDriver) -> (World, Arc<FakeClock>) {
    let mut w = world(driver.with_checkpoints());
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    w.cx = Cx::fake_with_clock(
        w.dir.path(),
        w.driver.clone(),
        w.surface.clone(),
        w.proc.clone(),
        UserConfig::default(),
        clock.clone(),
    )
    .with_forge(w.forge.clone());
    (w, clock)
}

fn days(n: u64) -> Duration {
    Duration::from_secs(n * 24 * 60 * 60)
}

fn forks(w: &World) -> usize {
    let calls = w.driver.calls();
    calls.iter().filter(|c| c.starts_with("fork")).count()
}

fn warned(w: &World, code: &str) -> usize {
    let all = w.cx.take_warnings();
    all.into_iter().filter(|x| x.code == code).count()
}

#[test]
fn a_chain_under_its_age_limit_is_forked() {
    let (w, clock) = timed(FakeDriver::new());
    start(&w, "first", "abc123", LINES);
    clock.sleep(days(6));
    let next = start(&w, "next", "abc123", LINES);
    assert_eq!(events(&w, next, "warm_started").len(), 1);
    assert_eq!(warned(&w, "checkpoint_chain_expired"), 0);
}

#[test]
fn a_chain_past_its_age_goes_cold_once_says_why_and_starts_over() {
    let (w, clock) = timed(FakeDriver::new());
    start(&w, "first", "abc123", LINES);
    clock.sleep(days(8));
    w.cx.take_warnings();
    let old = start(&w, "old", "abc123", LINES);
    assert!(events(&w, old, "warm_started").is_empty());
    assert_eq!(forks(&w), 0);
    assert_eq!(warned(&w, "checkpoint_chain_expired"), 1);
    let told = events(&w, old, "warm_start_failed");
    assert!(
        told[0]["error"].as_str().unwrap().contains("days"),
        "{told:?}"
    );
    assert_eq!(lines_on(&w, old), ["one-a"]);
    let next = start(&w, "next", "abc123", LINES);
    assert_eq!(events(&w, next, "warm_started").len(), 1, "a new chain");
    assert_eq!(warned(&w, "checkpoint_chain_expired"), 0);
    clock.sleep(days(6));
    let later = start(&w, "later", "abc123", LINES);
    assert_eq!(
        events(&w, later, "warm_started").len(),
        1,
        "its age starts over"
    );
}

#[test]
fn an_expired_chain_goes_from_the_disk_at_the_next_start() {
    let driver = FakeDriver::new().with_checkpoint_disk(100_000, 90_000, 10);
    let (w, clock) = timed(driver);
    start(&w, "first", "abc123", LINES);
    let old = w.driver.checkpoint_ids();
    clock.sleep(days(8));
    start(&w, "old", "abc123", LINES);
    start(&w, "next", "abc123", LINES);
    let ids = w.driver.checkpoint_ids();
    assert!(old.iter().all(|id| !ids.contains(id)), "{old:?} {ids:?}");
}

/// Replaces the top of Box `first`'s chain with a row `depth` rolls deep.
fn at_depth(w: &World, first: BoxId, depth: u32) {
    let taken = events(w, first, "checkpoint_taken");
    let key = taken.last().unwrap()["key"].as_str().unwrap().to_owned();
    let key = LayerKey::restore(key).unwrap();
    let clone = taken[0]["key"].as_str().unwrap().to_owned();
    let lineage = Lineage {
        user: crate::testing::cx::REQUESTER_USER,
        repo: "o/n".to_owned(),
        chain: LayerKey::restore(clone).unwrap(),
        depth,
        root_at: None,
    };
    let provider = row(w, first).boxed.unwrap().provider;
    let store = w.cx.store().unwrap();
    let handle = row(w, first).boxed.unwrap().handle.unwrap();
    let made = w.driver.checkpoint(&handle, key.as_str()).unwrap();
    store
        .record_checkpoint(&provider, &key, &lineage, &made)
        .unwrap();
    let place = Place {
        workdir: BoxPath::restore("/work/cbx_1/repo").unwrap(),
        setup_ms: 1,
        base: "abc123".to_owned(),
        stale_since: "abc123".to_owned(),
    };
    store.place_checkpoint(&provider, &key, &place).unwrap();
}

#[test]
fn a_chain_at_its_depth_limit_goes_cold_and_one_under_it_is_forked() {
    let (w, _) = timed(FakeDriver::new());
    let first = start(&w, "first", "abc123", LINES);
    at_depth(&w, first, 7);
    assert_eq!(
        events(&w, start(&w, "under", "abc123", LINES), "warm_started").len(),
        1
    );
    at_depth(&w, first, 8);
    w.cx.take_warnings();
    let at = start(&w, "at", "abc123", LINES);
    assert!(events(&w, at, "warm_started").is_empty());
    assert_eq!(warned(&w, "checkpoint_chain_expired"), 1);
}

fn cold_req(name: &str, base: &str, yaml: &str, w: &World) -> StartReq {
    w.forge.answer(repo(), Ok(repo_at(base, Some(yaml))));
    StartReq {
        repo: Some(repo()),
        cold: true,
        ..agentless_req(name)
    }
}

#[test]
fn a_cold_start_records_no_warm_start_and_replaces_what_it_went_past() {
    let (w, _) = timed(FakeDriver::new());
    start(&w, "first", "abc123", LINES);
    let saved = w.driver.checkpoint_ids();
    let req = cold_req("cold", "def456", LINES, &w);
    let cold = BoxId::new(run(&w.cx, &req).unwrap().boxed.id);
    assert!(events(&w, cold, "warm_started").is_empty());
    assert_eq!(forks(&w), 0);
    assert_eq!(lines_on(&w, cold), ["one-a"]);
    assert!(
        w.driver.checkpoint_ids().len() > saved.len(),
        "it saved its own"
    );
    let next = start(&w, "next", "def456", LINES);
    let warm = events(&w, next, "warm_started");
    assert!(warm[0].get("rerun_from").is_none(), "{warm:?}");
}

#[test]
fn a_retry_of_a_cold_box_never_forks() {
    let failing = ExecOutcome::Exited { code: 1 };
    let (w, _) = timed(FakeDriver::new().exec_ends_once_when("two-b", failing));
    start(&w, "first", "abc123", LINES);
    let req = cold_req("cold", "abc123", "setup: [one-a, two-b]\n", &w);
    let cold = BoxId::new(run(&w.cx, &req).unwrap().boxed.id);
    assert_eq!(row(&w, cold).lifecycle, BoxLifecycle::SetupFailed);
    let retry = BoxReq {
        box_name: crate::testing::box_name("cold"),
    };
    crate::ops::retry::run(&w.cx, &retry).unwrap();
    assert_eq!(forks(&w), 0);
    assert!(events(&w, cold, "warm_started").is_empty());
}

#[test]
fn a_cold_flag_on_a_box_that_exists_says_it_did_nothing() {
    let (w, _) = timed(FakeDriver::new());
    start(&w, "first", "abc123", LINES);
    w.cx.take_warnings();
    let req = cold_req("first", "abc123", LINES, &w);
    run(&w.cx, &req).unwrap();
    assert_eq!(warned(&w, "cold_ignored"), 1);
}
