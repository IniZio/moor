use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::task::TaskState;
use moor_core::request::{WaitEventsReq, WaitMode};
use moor_core::{BoxId, BoxName, ShowReq, StartReq};

use super::{req, world, world_at, World};
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::start::{run, settle_all};
use crate::ops::{show, wait_events};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// A Working Task with a goal, a ready Box and no Session.
fn working_without_session() -> (World, BoxId) {
    let w = world(FakeDriver::new());
    let start = StartReq {
        goal: Some("ship it".into()),
        no_agent: true,
        ..req("demo")
    };
    let id = BoxId::new(run(&w.cx, &start).unwrap().task.id);
    (w, id)
}

fn state(w: &World, id: BoxId) -> TaskState {
    w.cx.store().unwrap().task(id).unwrap().unwrap().state
}

#[test]
fn settle_leaves_a_task_while_an_op_holds_its_box() {
    let (w, id) = working_without_session();
    w.cx.clock().sleep(Duration::from_secs(60));
    let held = BoxLock::acquire(
        w.cx.clock(),
        w.cx.state_dir().unwrap(),
        id,
        "start",
        Duration::ZERO,
    )
    .unwrap();
    settle_all(&w.cx, w.cx.store().unwrap()).unwrap();
    assert_eq!(state(&w, id), TaskState::Working);
    drop(held);
    settle_all(&w.cx, w.cx.store().unwrap()).unwrap();
    assert_eq!(state(&w, id), TaskState::Idle);
}

/// Holds the Box of `id` as a start would, with the Task old enough to be read as quiet.
fn busy(w: &World, id: BoxId) -> BoxLock {
    w.cx.clock().sleep(Duration::from_secs(60));
    BoxLock::acquire(
        w.cx.clock(),
        w.cx.state_dir().unwrap(),
        id,
        "start",
        Duration::ZERO,
    )
    .unwrap()
}

#[test]
fn show_leaves_a_task_while_an_op_holds_its_box() {
    let (w, id) = working_without_session();
    let _held = busy(&w, id);
    let list = ShowReq {
        task: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    };
    show::run(&w.cx, &list).unwrap();
    assert_eq!(state(&w, id), TaskState::Working);
}

#[test]
fn wait_events_leaves_a_task_while_an_op_holds_its_box() {
    let (w, id) = working_without_session();
    let _held = busy(&w, id);
    let wait = WaitEventsReq {
        boxes: vec![BoxName::parse_allow_bare("demo").unwrap()],
        kinds: None,
        mode: WaitMode::Any,
        since: None,
        timeout_secs: Some(0),
    };
    let _ = wait_events::run(&w.cx, &wait);
    assert_eq!(state(&w, id), TaskState::Working);
}

/// Happy path only: settles run during setup, before the Task is ready, and the agent launches.
#[test]
fn a_start_with_settles_running_during_setup_still_launches() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [a]\n").unwrap();
    let mut w = world_at(FakeDriver::new(), repo.path());
    let engine = Arc::new(FakeEngine::new());
    w.cx = w.cx.with_engine(engine.clone());
    let (dir, cwd) = (w.dir.path().to_path_buf(), repo.path().to_path_buf());
    let (driver, surface, proc) = (w.driver.clone(), w.surface.clone(), w.proc.clone());
    w.driver.after_exec(move || {
        let other = Cx::fake_with(
            &dir,
            &cwd,
            driver.clone(),
            surface.clone(),
            proc.clone(),
            crate::testing::cx::fake_config(),
        );
        other.clock().sleep(Duration::from_secs(60));
        settle_all(&other, other.store().unwrap()).unwrap();
    });
    let start = StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    };
    let v = run(&w.cx, &start).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
    let id = BoxId::new(v.task.id);
    assert!(w.cx.store().unwrap().has_session(id).unwrap());
}
