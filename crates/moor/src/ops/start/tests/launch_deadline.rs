//! A Box whose agent never launched reads quiet once `box.launch_timeout` has passed.

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::{parse_user, UserConfig};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{BoxId, StartReq};

use super::dead_agent::ca_stopped;
use super::relaunch::session;
use super::{req, world_with, World};
use crate::ops::common::activity::{read, settle, PROBE_DEADLINE};
use crate::ops::start::run;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

fn prompt_req(name: &str) -> StartReq {
    StartReq {
        prompt: Some("ship it".into()),
        ..req(name)
    }
}

/// A Box whose agent exited at launch, and the clock the world runs on.
fn failed_launch(config: UserConfig) -> (World, Arc<FakeClock>, BoxId) {
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(session("p-1"))));
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let mut w = world_with(FakeDriver::new(), config);
    w.cx = w.cx.with_engine(engine).with_clock(clock.clone());
    w.surface.push_pane_obs("p-1", Ok(ca_stopped()));
    let v = run(&w.cx, &prompt_req("demo")).unwrap();
    assert!(!v.agent.launched);
    (w, clock, BoxId::new(v.boxed.id))
}

/// Reads the agent of Box `id` and settles the Box on it, as every read of a Box does.
fn settle_now(w: &World, id: BoxId) {
    let store = w.cx.store().unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    let reading = read(&w.cx, store, &row, w.cx.clock().now(), PROBE_DEADLINE).unwrap();
    settle(&w.cx, store, &row, &reading).unwrap();
}

fn timed_out(w: &World, id: BoxId) -> Vec<serde_json::Value> {
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    events
        .into_iter()
        .filter(|e| e.kind == "launch_timed_out")
        .map(|e| e.payload)
        .collect()
}

fn quiet(w: &World, id: BoxId) -> bool {
    w.cx.store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .session
        .is_quiet()
}

#[test]
fn a_failed_launch_is_not_quiet_before_the_timeout() {
    let (w, clock, id) = failed_launch(UserConfig::default());
    clock.sleep(Duration::from_mins(2));
    settle_now(&w, id);
    assert!(!quiet(&w, id));
    assert!(timed_out(&w, id).is_empty());
}

#[test]
fn a_failed_launch_reads_quiet_after_the_default_three_minutes_and_says_so_once() {
    let (w, clock, id) = failed_launch(UserConfig::default());
    clock.sleep(Duration::from_mins(4));
    settle_now(&w, id);
    assert!(quiet(&w, id));
    assert_eq!(timed_out(&w, id), [serde_json::json!({"after_secs": 180})]);
    let warnings = w.cx.take_warnings();
    let warned = warnings.iter().find(|x| x.code == "launch_timed_out");
    assert!(
        warned.is_some_and(|x| x.message.contains("moor start demo --resume")),
        "{warnings:?}"
    );
    settle_now(&w, id);
    assert_eq!(timed_out(&w, id).len(), 1, "a second read adds no event");
}

#[test]
fn the_launch_timeout_is_configurable() {
    let config = parse_user("config.yaml", "box: {launch_timeout: 1m}").unwrap();
    let (w, clock, id) = failed_launch(config);
    clock.sleep(Duration::from_secs(90));
    settle_now(&w, id);
    assert!(quiet(&w, id));
    assert_eq!(timed_out(&w, id), [serde_json::json!({"after_secs": 60})]);
}
