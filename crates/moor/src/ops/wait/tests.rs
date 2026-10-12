//! Tests for `wait`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::{ErrorKind, ShowReq, TaskId, TaskName, WaitFor, WaitReq};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::store::NewRun;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const PID: u32 = 4242;

/// Context with state under `dir`, plus the scripted proc behind it.
fn cx(dir: &Path) -> (Cx, Arc<ScriptedProc>) {
    let proc = Arc::new(ScriptedProc::new());
    let cx = Cx::fake(
        dir,
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(dir)),
        proc.clone(),
    );
    (cx, proc)
}

/// Insert task `demo`.
fn add(cx: &Cx) -> TaskId {
    let nt = NewTask {
        name: TaskName::parse("demo").unwrap(),
        repo: "r".into(),
    };
    cx.store()
        .unwrap()
        .apply(Decision::insert(nt))
        .unwrap()
        .task
}

/// Begin a run of `demo` owned by `PID`.
fn begin(cx: &Cx, task: TaskId) {
    cx.store()
        .unwrap()
        .begin_run(NewRun {
            task,
            argv: vec!["make".into(), "test".into()],
            record: None,
            sha: None,
            sync: "none".into(),
            pid: PID,
        })
        .unwrap();
}

/// A `wait` request for `demo`.
fn req(wait_for: Option<WaitFor>, timeout_secs: Option<u64>) -> WaitReq {
    WaitReq {
        task: TaskName::parse("demo").unwrap(),
        wait_for,
        timeout_secs,
    }
}

#[test]
fn returns_at_once_without_agent_or_run() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    add(&cx);
    let v = run(&cx, &req(None, None)).unwrap();
    assert!(!v.timed_out);
    assert_eq!(v.waited_ms, 0);
    assert_eq!(v.wait_for, WaitFor::Idle);
    assert!(v.run.is_none());
}

#[test]
fn times_out_while_a_run_is_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let id = add(&cx);
    begin(&cx, id);
    let before = cx.clock().now();
    let v = run(&cx, &req(None, Some(3))).unwrap();
    assert!(v.timed_out);
    let slept = cx.clock().now().as_millis() - before.as_millis();
    assert!((3000..=5000).contains(&slept), "slept {slept} ms");
}

#[test]
fn a_short_timeout_caps_the_total_sleep() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let id = add(&cx);
    begin(&cx, id);
    let before = cx.clock().now();
    let v = run(&cx, &req(None, Some(1))).unwrap();
    assert!(v.timed_out);
    let slept = cx.clock().now().as_millis() - before.as_millis();
    assert!(slept <= 3000, "slept {slept} ms");
}

#[test]
fn reports_the_in_flight_run_with_its_argv() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let id = add(&cx);
    begin(&cx, id);
    let v = run(&cx, &req(None, Some(1))).unwrap();
    let r = v.run.unwrap();
    assert!(r.in_flight);
    assert_eq!(r.argv, ["make", "test"]);
    assert!(v.task.run.is_some());
}

#[test]
fn a_run_whose_process_is_gone_reports_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, proc) = cx(dir.path());
    let id = add(&cx);
    begin(&cx, id);
    proc.dead(PID);
    let v = run(&cx, &req(None, Some(5))).unwrap();
    assert!(!v.timed_out);
    let r = v.run.unwrap();
    assert!(!r.in_flight);
    assert!(r.interrupted);
    assert_eq!(r.argv, ["make", "test"]);
    assert!(v.task.run.is_none());
}

#[test]
fn done_behaves_like_idle() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let id = add(&cx);
    begin(&cx, id);
    let v = run(&cx, &req(Some(WaitFor::Done), Some(1))).unwrap();
    assert!(v.timed_out);
    assert_eq!(v.wait_for, WaitFor::Done);
}

#[test]
fn run_waits_only_for_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, proc) = cx(dir.path());
    let id = add(&cx);
    let v = run(&cx, &req(Some(WaitFor::Run), None)).unwrap();
    assert!(!v.timed_out);
    begin(&cx, id);
    let v = run(&cx, &req(Some(WaitFor::Run), Some(1))).unwrap();
    assert!(v.timed_out);
    proc.dead(PID);
    let v = run(&cx, &req(Some(WaitFor::Run), Some(1))).unwrap();
    assert!(!v.timed_out);
}

#[test]
fn unknown_task_fails_like_show() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let err = run(&cx, &req(None, None)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(err.error.to_string(), "no task demo");
}

#[test]
fn never_takes_the_box_lock() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, proc) = cx(dir.path());
    let id = add(&cx);
    let _held = BoxLock::acquire(
        cx.clock(),
        cx.state_dir().unwrap(),
        id,
        "close",
        Duration::ZERO,
    )
    .unwrap();
    let show = ShowReq {
        task: Some(TaskName::parse("demo").unwrap()),
        events: None,
        agent: false,
        reports: false,
        pane: true,
    };
    let began = cx.clock().now();
    crate::ops::show::run(&cx, &show).unwrap();
    run(&cx, &req(None, None)).unwrap();
    begin(&cx, id);
    run(&cx, &req(None, Some(1))).unwrap();
    proc.dead(PID);
    assert!(cx.clock().now().as_millis() - began.as_millis() < 8000);
}
