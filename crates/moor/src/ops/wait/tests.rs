//! Tests for `wait`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, EgressPlan};
use moor_core::{ErrorKind, ProviderId, ShowReq, TaskId, TaskName, WaitFor, WaitReq};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::store::NewRun;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const PID: u32 = 4242;
const SESSION: &str = "sess-1";

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

/// Context whose engine answers `probes` for the live session.
fn agent_cx(dir: &Path, probes: Probes) -> (Cx, Arc<FakeEngine>) {
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let (cx, _) = cx(dir);
    (cx.with_engine(engine.clone()), engine)
}

/// Insert task `demo` with a Box the driver lists as live and a live agent session.
fn add_live(cx: &Cx) -> TaskId {
    let id = add(cx);
    let got = cx
        .driver()
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: id,
            name: TaskName::parse("demo").unwrap(),
            egress: EgressPlan::Open,
            class: None,
            nonce: "abc123".into(),
            worktree: None,
        })
        .unwrap();
    let store = cx.store().unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            worktree: Field::Set("/wt".into()),
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    store.begin_session(id, SESSION).unwrap();
    id
}

/// Probes with a fresh hook report of `kind` and an unreadable pane status.
fn hook(kind: HookKind) -> Probes {
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind,
            session_id: None,
            at: Ts::from_millis(0),
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            exit_marker: None,
        }),
    }
}

/// Probes that learn nothing from the hook or the pane.
fn silent(hook: HookRead, pane: PaneRead) -> Probes {
    Probes { hook, pane }
}

#[test]
fn a_busy_agent_keeps_idle_waiting_until_timeout() {
    let cases = [
        ("working", hook(HookKind::Busy)),
        ("starting", silent(HookRead::Absent, PaneRead::Absent)),
        ("unknown", silent(HookRead::TimedOut, PaneRead::TimedOut)),
    ];
    for (name, probes) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (cx, _) = agent_cx(dir.path(), probes);
        add_live(&cx);
        let before = cx.clock().now();
        let v = run(&cx, &req(None, Some(1))).unwrap();
        assert!(v.timed_out, "{name}");
        let slept = cx.clock().now().as_millis() - before.as_millis();
        assert!((1000..=3000).contains(&slept), "{name}: slept {slept} ms");
    }
}

#[test]
fn an_idle_or_blocked_agent_returns_at_once() {
    let blocked = HookKind::Blocked {
        waiting_for: "Allow edit?".into(),
    };
    for kind in [HookKind::Idle, blocked] {
        let dir = tempfile::tempdir().unwrap();
        let (cx, _) = agent_cx(dir.path(), hook(kind));
        add_live(&cx);
        let v = run(&cx, &req(None, Some(5))).unwrap();
        assert!(!v.timed_out);
        assert_eq!(v.waited_ms, 0);
    }
}

#[test]
fn the_payload_carries_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = HookKind::Blocked {
        waiting_for: "Allow edit?".into(),
    };
    let (cx, _) = agent_cx(dir.path(), hook(blocked));
    add_live(&cx);
    let v = run(&cx, &req(None, None)).unwrap();
    let a = v.task.agent.expect("an agent view");
    assert_eq!(a.state, "blocked");
    assert_eq!(a.source, "hooks");
    assert_eq!(a.pane, "live");
    assert_eq!(a.waiting_for.as_deref(), Some("Allow edit?"));
}

#[test]
fn a_task_without_a_session_has_no_agent_in_the_payload() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    add(&cx);
    let v = run(&cx, &req(None, None)).unwrap();
    assert!(v.task.agent.is_none());
}

#[test]
fn waiting_leaves_the_task_row_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = agent_cx(dir.path(), hook(HookKind::Idle));
    let id = add_live(&cx);
    let before = cx.store().unwrap().task(id).unwrap().unwrap();
    for _ in 0..3 {
        let v = run(&cx, &req(None, Some(1))).unwrap();
        assert!(!v.timed_out);
        assert_eq!(cx.store().unwrap().task(id).unwrap().unwrap(), before);
    }
}

#[test]
fn each_poll_probes_the_agent_once() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, engine) = agent_cx(dir.path(), hook(HookKind::Idle));
    add_live(&cx);
    run(&cx, &req(None, None)).unwrap();
    assert_eq!(engine.probe_count(SESSION), 1);
}

#[test]
fn a_working_agent_is_probed_on_every_poll() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, engine) = agent_cx(dir.path(), hook(HookKind::Busy));
    add_live(&cx);
    let before = cx.clock().now();
    let v = run(&cx, &req(None, Some(1))).unwrap();
    assert!(v.timed_out);
    let polls = usize::try_from((cx.clock().now().as_millis() - before.as_millis()) / 250).unwrap();
    assert!(polls >= 4, "polled {polls} times");
    assert_eq!(engine.probe_count(SESSION), polls + 1);
}
