use moor_core::config::Flags;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::{BoxId, StartReq};
use std::path::Path;

use super::relaunch::{gone, started};
use super::state::idle as is_idle;
use super::{req, world, World};
use crate::ops::common::facts::settings;
use crate::ops::start::launch;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

/// An Idle Box with a goal, a ready machine and no Session.
fn idle() -> (World, BoxId) {
    let w = world(FakeDriver::new());
    let start = StartReq {
        goal: Some("ship it".into()),
        no_agent: true,
        ..req("demo")
    };
    let id = BoxId::new(run(&w.cx, &start).unwrap().boxed.id);
    let quiet = Decision::new(id).session(SessionInput::AgentQuiet);
    w.cx.store().unwrap().apply(quiet).unwrap();
    (w, id)
}

fn launch_with(w: &World, id: BoxId, start: &StartReq) -> moor_core::view::AgentLaunchView {
    let cfg = settings(&w.cx, Path::new("/repo"), &Flags::default()).unwrap();
    launch::run(&w.cx, w.cx.store().unwrap(), start, id, None, &|| {
        Ok(cfg.clone())
    })
    .unwrap()
}

#[test]
fn a_launch_skipped_because_the_box_is_not_working_is_recorded() {
    let (w, id) = idle();
    let start = StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    };
    let view = launch_with(&w, id, &start);
    assert!(!view.launched);
    let failure = view.reason.unwrap();
    assert_eq!(failure.step, "launch");
    assert!(failure.message.contains("idle"), "{}", failure.message);
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert!(is_idle(&row));
    assert!(row.launch_failure.is_some());
}

#[test]
fn a_held_or_agentless_start_that_does_not_launch_stays_silent() {
    let (w, id) = idle();
    for start in [
        StartReq {
            goal: Some("ship it".into()),
            hold: true,
            ..req("demo")
        },
        StartReq {
            goal: Some("ship it".into()),
            no_agent: true,
            ..req("demo")
        },
    ] {
        let view = launch_with(&w, id, &start);
        assert!(!view.launched && view.reason.is_none());
    }
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert!(row.launch_failure.is_none());
}

#[test]
fn no_agent_beats_a_goal_on_a_gone_pane() {
    let (w, engine, _id) = started();
    gone(&w);
    let r = StartReq {
        goal: Some("ship it".into()),
        no_agent: true,
        ..req("demo")
    };
    let v = run(&w.cx, &r).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
}
