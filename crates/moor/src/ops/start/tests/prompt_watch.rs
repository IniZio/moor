use std::sync::Arc;

use moor_core::activity::Probe;
use moor_core::seam::agent_adapter::FirstRunPrompt;
use moor_core::seam::clock::Ts;
use moor_core::StartReq;

use super::{req, world, World};
use crate::ops::start::run;
use crate::testing::agent;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// The agent session the first Box's launch records.
const SESSION: &str = "moor-1";

/// Reads of a watch that runs its whole window.
const FULL_WINDOW: usize = 17;

fn prompt_req(name: &str) -> StartReq {
    StartReq {
        prompt: Some("ship it".into()),
        ..req(name)
    }
}

/// A read of an agent that has reported no status, its screen showing `screen`.
fn silent(screen: &str) -> Probe {
    agent::on_screen(agent::read(None, Ts::from_millis(1_000), 0), screen)
}

/// A world whose engine knows the folder-trust prompt.
fn watching(engine: FakeEngine) -> (World, Arc<FakeEngine>) {
    let engine = Arc::new(engine.prompt("trust this folder", FirstRunPrompt::FolderTrust));
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    (w, engine)
}

#[test]
fn a_status_report_ends_the_watch_early() {
    let engine = FakeEngine::new()
        .then_probes(SESSION, silent("working"))
        .then_probes(SESSION, silent("working"))
        .probes(SESSION, agent::working(Ts::from_millis(1_000)));
    let (w, engine) = watching(engine);
    let v = run(&w.cx, &prompt_req("demo")).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.probe_count(SESSION), 3);
}

#[test]
fn an_idle_status_ends_the_watch_as_well() {
    let engine = FakeEngine::new()
        .then_probes(SESSION, silent("working"))
        .probes(SESSION, agent::idle(Ts::from_millis(1_000)));
    let (w, engine) = watching(engine);
    run(&w.cx, &prompt_req("demo")).unwrap();
    assert_eq!(engine.probe_count(SESSION), 2);
}

#[test]
fn a_status_on_the_first_read_ends_the_watch_at_once() {
    let engine = FakeEngine::new().probes(SESSION, agent::working(Ts::from_millis(1_000)));
    let (w, engine) = watching(engine);
    run(&w.cx, &prompt_req("demo")).unwrap();
    assert_eq!(engine.probe_count(SESSION), 1);
}

#[test]
fn an_agent_that_reports_no_status_keeps_the_full_window() {
    let engine = FakeEngine::new().probes(SESSION, silent("working"));
    let (w, engine) = watching(engine);
    run(&w.cx, &prompt_req("demo")).unwrap();
    assert_eq!(engine.probe_count(SESSION), FULL_WINDOW);
}

#[test]
fn a_prompt_on_screen_is_recorded_before_a_status_ends_the_watch() {
    let engine = FakeEngine::new()
        .then_probes(SESSION, silent("Do you trust this folder?"))
        .probes(SESSION, agent::working(Ts::from_millis(1_000)));
    let (w, engine) = watching(engine);
    let v = run(&w.cx, &prompt_req("demo")).unwrap();
    let blocked =
        w.cx.store()
            .unwrap()
            .events(moor_core::BoxId::new(v.boxed.id), 50)
            .unwrap()
            .into_iter()
            .any(|e| e.kind == "agent_prompt_blocked");
    assert!(blocked);
    assert_eq!(engine.probe_count(SESSION), 1);
}
