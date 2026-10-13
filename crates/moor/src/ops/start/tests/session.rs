use moor_core::domain::task::TaskState;
use moor_core::{StartReq, TaskId, TaskName, WaitFor, WaitReq};

use super::{req, world};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

/// A start request for `name` with a goal.
fn goal_req(name: &str) -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req(name)
    }
}

#[test]
fn start_records_one_live_session_and_attach_adds_none() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &goal_req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    let store = w.cx.store().unwrap();
    let sessions = || -> i64 {
        store
            .reader()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE task = ? AND ended_at IS NULL",
                [id.get()],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(first.task.state, TaskState::Working);
    assert_eq!(sessions(), 1);
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_eq!(sessions(), 1);
}

#[test]
fn start_without_a_goal_records_no_session() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(first.task.state, TaskState::Working);
    assert!(!w.cx.store().unwrap().has_session(id).unwrap());
}

#[test]
fn wait_for_idle_returns_after_a_start_without_a_goal() {
    let w = world(FakeDriver::new());
    run(&w.cx, &req("demo")).unwrap();
    let wait = WaitReq {
        task: TaskName::parse("demo").unwrap(),
        wait_for: Some(WaitFor::Idle),
        timeout_secs: Some(60),
    };
    let v = crate::ops::wait::run(&w.cx, &wait).unwrap();
    assert!(!v.timed_out);
    assert_eq!(v.waited_ms, 0);
}
