use moor_core::seam::engine::LaunchPrompt;
use moor_core::StartReq;

use super::relaunch::{gone, started};
use super::{req, world};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use std::sync::Arc;

fn held(goal: Option<&str>) -> StartReq {
    StartReq {
        goal: goal.map(Into::into),
        hold: true,
        ..req("demo")
    }
}

#[test]
fn a_held_start_launches_without_a_prompt_and_keeps_the_goal() {
    let engine = Arc::new(FakeEngine::new());
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let v = run(&w.cx, &held(Some("ship it"))).unwrap();
    let all = engine.launches();
    assert_eq!(all.len(), 1);
    let first = all.first().unwrap();
    assert_eq!(first.prompt, LaunchPrompt::None);
    assert_eq!(first.goal.as_deref(), Some("ship it"));
    let row =
        w.cx.store()
            .unwrap()
            .task(moor_core::BoxId::new(v.task.id))
            .unwrap()
            .unwrap();
    assert_eq!(row.goal.as_deref(), Some("ship it"));
}

#[test]
fn a_start_without_hold_launches_with_the_brief() {
    let (_w, engine, _id) = started();
    assert_eq!(
        engine.launches().first().unwrap().prompt,
        LaunchPrompt::Brief
    );
}

#[test]
fn a_held_relaunch_launches_without_a_prompt() {
    let (w, engine, _id) = started();
    gone(&w);
    run(&w.cx, &held(None)).unwrap();
    let all = engine.launches();
    assert_eq!(all.len(), 2);
    let second = all.get(1).unwrap();
    assert_eq!(second.prompt, LaunchPrompt::None);
    assert_eq!(second.goal.as_deref(), Some("ship it"));
}

#[test]
fn a_relaunch_without_hold_uses_the_brief() {
    let (w, engine, _id) = started();
    gone(&w);
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(
        engine.launches().get(1).unwrap().prompt,
        LaunchPrompt::Brief
    );
}
