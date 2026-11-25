//! A Box a place's agent started is tied to that place and runs no agent, woken or not.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::{BoxId, BoxName, CloseReq, ErrorKind, StartReq};

use super::{req, world, World};
use crate::ops::close;
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
fn a_start_for_a_task_attaches_only_to_a_box_that_task_started() {
    let (w, _engine, _id) = asleep_task_box();
    let store = w.cx.store().unwrap();
    let place = store.box_row(BoxId::new(1)).unwrap().unwrap().id;
    let again = StartReq {
        no_agent: true,
        task: Some(place),
        ..req("w")
    };
    assert!(run(&w.cx, &again).unwrap().attached);
    let other = store
        .apply(new_idle_place(BoxName::parse_allow_bare("q").unwrap()))
        .unwrap()
        .box_id;
    let theirs = StartReq {
        task: Some(other),
        ..again
    };
    let why = run(&w.cx, &theirs).unwrap_err();
    assert_eq!(why.kind, ErrorKind::Refused);
    let said = why.error.to_string();
    assert!(said.contains("this task did not start it"), "{said}");
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

/// A start of `name` for place `task`, launching no agent.
fn for_task(name: &str, task: BoxId) -> StartReq {
    StartReq {
        no_agent: true,
        task: Some(task),
        ..req(name)
    }
}

/// A place named `name` in `w`.
fn place(w: &World, name: &str) -> BoxId {
    w.cx.store()
        .unwrap()
        .apply(new_idle_place(BoxName::parse_allow_bare(name).unwrap()))
        .unwrap()
        .box_id
}

/// The branch Box `name` started on, after a start of it.
fn branch_of(w: &World, req: &StartReq) -> String {
    run(&w.cx, req).unwrap().boxed.branch.unwrap()
}

fn close_box(w: &World, name: &str) {
    let req = CloseReq {
        target: BoxName::parse_allow_bare(name).unwrap().into(),
        force: true,
    };
    close::run(&w.cx, &req).unwrap();
}

#[test]
fn a_later_box_of_a_task_starts_on_the_branch_of_its_first_box() {
    let w = world(FakeDriver::new());
    let p = place(&w, "p");
    let first = branch_of(&w, &for_task("w", p));
    close_box(&w, "w");
    assert_eq!(branch_of(&w, &for_task("w2", p)), first);
    close_box(&w, "w2");
    assert_eq!(branch_of(&w, &for_task("w", p)), first);
}

#[test]
fn a_box_of_another_task_or_of_none_gets_a_branch_of_its_own() {
    let w = world(FakeDriver::new());
    let (p, q) = (place(&w, "p"), place(&w, "q"));
    let first = branch_of(&w, &for_task("w", p));
    close_box(&w, "w");
    let theirs = run(&w.cx, &for_task("x", q)).unwrap().boxed;
    assert_eq!(theirs.branch, Some(format!("moor/{}-x", theirs.id)));
    let plain = run(
        &w.cx,
        &StartReq {
            no_agent: true,
            ..req("y")
        },
    )
    .unwrap()
    .boxed;
    assert_eq!(plain.branch, Some(format!("moor/{}-y", plain.id)));
    assert_ne!(theirs.branch.as_deref(), Some(first.as_str()));
}

#[test]
fn a_second_box_while_the_task_s_box_is_open_is_refused_naming_it() {
    let w = world(FakeDriver::new());
    let p = place(&w, "p");
    run(&w.cx, &for_task("w", p)).unwrap();
    let why = run(&w.cx, &for_task("w2", p)).unwrap_err();
    assert_eq!(why.kind, ErrorKind::Refused);
    let said = why.error.to_string();
    for want in ["Box w is still open", "exec_in_box", "close_box"] {
        assert!(said.contains(want), "missing {want:?} in {said}");
    }
    assert!(w
        .cx
        .store()
        .unwrap()
        .open_box_by_name(&BoxName::parse_allow_bare("w2").unwrap())
        .unwrap()
        .is_none());
}
