//! A Box a place's agent started is tied to that place and runs no agent, woken or not.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::{BoxId, BoxName, StartReq};

use super::{req, world, World};
use crate::ops::start::revive::revivable;
use crate::ops::start::run;
use crate::testing::cx::new_idle_place;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// The place and a Box it started, asleep, in a world whose engine would launch an agent.
fn asleep_task_box() -> (World, Arc<FakeEngine>, BoxId) {
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(super::relaunch::session("p-1"))));
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let store = w.cx.store().unwrap();
    let place = store
        .apply(new_idle_place(BoxName::parse_allow_bare("p").unwrap()))
        .unwrap()
        .box_id;
    let start = StartReq {
        no_agent: true,
        task: Some(place),
        ..req("w")
    };
    let id = BoxId::new(run(&w.cx, &start).unwrap().boxed.id);
    assert_eq!(store.box_row(id).unwrap().unwrap().task, Some(place));
    store
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Sleep {
                    auto: true,
                    reason: None,
                })
                .hands_input(HandsInc::FIRST, HandsInput::Sleep),
        )
        .unwrap();
    (w, engine, id)
}

#[test]
fn a_task_box_woken_launches_no_agent() {
    let (w, engine, id) = asleep_task_box();
    let asked = StartReq {
        prompt: Some("carry on".into()),
        ..req("w")
    };
    let v = run(&w.cx, &asked).unwrap();
    assert!(v.attached);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Ready);
    assert!(w.driver.calls().iter().any(|c| c.starts_with("wake ")));
    assert!(!v.agent.launched, "{:?}", v.agent);
    assert!(engine.launches().is_empty());
    let store = w.cx.store().unwrap();
    assert!(store.live_launch(id).unwrap().is_none());
    assert!(!revivable(&store.box_row(id).unwrap().unwrap()));
}
