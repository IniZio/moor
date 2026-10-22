use std::sync::Arc;
use std::time::Duration;

use moor_core::request::{WaitEventsReq, WaitMode};
use moor_core::{BoxId, BoxName, ShowReq, StartReq};

use super::state::{idle, working};
use super::{repo_req, req, script_base_yaml, world, World};
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::start::{run, settle_all};
use crate::ops::{show, wait_events};
use crate::store::BoxRow;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// A Working Box with a goal, a ready machine and no Session.
fn working_without_session() -> (World, BoxId) {
    let w = world(FakeDriver::new());
    let start = StartReq {
        goal: Some("ship it".into()),
        no_agent: true,
        ..req("demo")
    };
    let id = BoxId::new(run(&w.cx, &start).unwrap().boxed.id);
    (w, id)
}

fn row(w: &World, id: BoxId) -> BoxRow {
    w.cx.store().unwrap().box_row(id).unwrap().unwrap()
}

#[test]
fn settle_leaves_a_box_while_an_op_holds_its_box() {
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
    settle_all(&w.cx, w.cx.store().unwrap(), &|| false).unwrap();
    assert!(working(&row(&w, id)));
    drop(held);
    settle_all(&w.cx, w.cx.store().unwrap(), &|| false).unwrap();
    assert!(idle(&row(&w, id)));
}

/// Holds the machine of `id` as a start would, with the Box old enough to be read as quiet.
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
fn show_leaves_a_box_while_an_op_holds_its_box() {
    let (w, id) = working_without_session();
    let _held = busy(&w, id);
    let list = ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    };
    show::run(&w.cx, &list).unwrap();
    assert!(working(&row(&w, id)));
}

#[test]
fn wait_events_leaves_a_box_while_an_op_holds_its_box() {
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
    assert!(working(&row(&w, id)));
}

/// Happy path only: settles run during setup, before the Box is ready, and the agent launches.
#[test]
fn a_start_with_settles_running_during_setup_still_launches() {
    let mut w = world(FakeDriver::new());
    script_base_yaml(&w, Some("setup: [a]\n"));
    let engine = Arc::new(FakeEngine::new());
    w.cx = w.cx.with_engine(engine.clone());
    let dir = w.dir.path().to_path_buf();
    let (driver, surface, proc) = (w.driver.clone(), w.surface.clone(), w.proc.clone());
    w.driver.after_exec(move || {
        let other = Cx::fake_with(
            &dir,
            driver.clone(),
            surface.clone(),
            proc.clone(),
            moor_core::config::UserConfig::default(),
        );
        other.clock().sleep(Duration::from_secs(60));
        settle_all(&other, other.store().unwrap(), &|| false).unwrap();
    });
    let start = StartReq {
        goal: Some("ship it".into()),
        ..repo_req("demo")
    };
    let v = run(&w.cx, &start).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
    let id = BoxId::new(v.boxed.id);
    assert!(w.cx.store().unwrap().has_session(id).unwrap());
}

#[test]
fn start_idles_a_quiet_box_without_reading_its_artifacts() {
    let (w, id) = working_without_session();
    w.cx.clock().sleep(Duration::from_secs(60));
    let other = StartReq {
        no_agent: true,
        ..req("other")
    };
    run(&w.cx, &other).unwrap();
    assert!(idle(&row(&w, id)));
    let reads_artifacts = w
        .driver
        .execs()
        .iter()
        .any(|(_, spec, _)| spec.argv.iter().any(|a| a.contains(".moor/artifacts")));
    assert!(!reads_artifacts);
    let due = w.cx.store().unwrap().artifacts_due().unwrap();
    assert!(due.iter().any(|(due_id, _)| *due_id == id), "{due:?}");
}
