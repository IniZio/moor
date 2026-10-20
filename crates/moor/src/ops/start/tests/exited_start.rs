use std::sync::Arc;

use moor_core::activity::{HookRead, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::{BoxId, StartReq};

use super::relaunch::session;
use super::state::working;
use super::{req, world, World};
use crate::ops::start::{run, settle_all};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

fn exited() -> Probes {
    Probes {
        hook: HookRead::Absent,
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: Some(1),
        }),
    }
}

/// A Box whose agent started, exited and was settled, so its Session 1 is Ended.
fn exited_agent() -> (World, Arc<FakeEngine>, BoxId) {
    let named = |n: &str| {
        let mut s = session(&format!("p-{n}"));
        s.id = format!("sess-{n}");
        s
    };
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(named("1")))
            .launch_result(Ok(named("2")))
            .probes("sess-1", exited()),
    );
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let first = run(
        &w.cx,
        &StartReq {
            goal: Some("ship it".into()),
            ..req("demo")
        },
    )
    .unwrap();
    let id = BoxId::new(first.boxed.id);
    settle_all(&w.cx, w.cx.store().unwrap()).unwrap();
    let store = w.cx.store().unwrap();
    assert!(store.live_session(id).unwrap().is_none());
    (w, engine, id)
}

/// Each Session's ordinal and state, and the Session each launch row points at.
fn sessions(w: &World, id: BoxId) -> (Vec<(i64, String)>, Vec<Option<i64>>) {
    let conn = w.cx.store().unwrap().reader().unwrap();
    let rows = conn
        .prepare("SELECT ordinal, state FROM sessions WHERE box = ? ORDER BY ordinal")
        .unwrap()
        .query_map([id.get()], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let launches = conn
        .prepare("SELECT session FROM launches WHERE box = ? ORDER BY id")
        .unwrap()
        .query_map([id.get()], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    (rows, launches)
}

fn check_second_session(w: &World, engine: &FakeEngine, id: BoxId) {
    assert_eq!(engine.launches().len(), 2);
    let (rows, launches) = sessions(w, id);
    assert_eq!(rows, [(1, "ended".to_owned()), (2, "working".to_owned())]);
    let second: i64 =
        w.cx.store()
            .unwrap()
            .box_sessions(id)
            .unwrap()
            .last()
            .unwrap()
            .id;
    assert_eq!(launches.last().copied().flatten(), Some(second));
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert!(working(&row));
}

#[test]
fn a_goal_start_after_the_agent_exited_opens_a_new_session() {
    let (w, engine, id) = exited_agent();
    run(
        &w.cx,
        &StartReq {
            goal: Some("again".into()),
            ..req("demo")
        },
    )
    .unwrap();
    check_second_session(&w, &engine, id);
}

#[test]
fn a_resume_start_after_the_agent_exited_opens_a_new_session() {
    let (w, engine, id) = exited_agent();
    super::relaunch::set_agent_session(&w, id, "agent-1");
    run(
        &w.cx,
        &StartReq {
            resume: true,
            ..req("demo")
        },
    )
    .unwrap();
    check_second_session(&w, &engine, id);
}

#[test]
fn a_wake_after_the_agent_exited_opens_a_new_session() {
    let (w, engine, id) = exited_agent();
    crate::ops::start::revive_after_wake(&w.cx, w.cx.store().unwrap(), id).unwrap();
    check_second_session(&w, &engine, id);
}

#[test]
fn a_new_session_after_wake_makes_no_box_call() {
    let (w, engine, id) = exited_agent();
    let before = w.driver.calls();
    crate::ops::start::revive_after_wake(&w.cx, w.cx.store().unwrap(), id).unwrap();
    check_second_session(&w, &engine, id);
    assert_eq!(w.driver.calls(), before);
}
