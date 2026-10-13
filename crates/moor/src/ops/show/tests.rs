//! Tests for `show` and `reconcile`.

use std::path::Path;
use std::sync::Arc;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan};
use moor_core::{ProviderId, ShowReq, TaskId, TaskName};

use super::run;
use crate::cx::Cx;
use crate::store::NewRun;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

/// Context over `driver` with state under `dir`.
fn cx(dir: &Path, driver: &Arc<FakeDriver>) -> Cx {
    Cx::fake(
        dir,
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new(dir)),
        Arc::new(ScriptedProc::new()),
    )
}

/// Request showing everything.
fn req(task: Option<&str>, events: Option<u32>) -> ShowReq {
    ShowReq {
        task: task.map(|t| TaskName::parse(t).unwrap()),
        events,
        agent: false,
        reports: false,
        pane: false,
    }
}

/// Insert a task by name.
fn add(cx: &Cx, name: &str) -> TaskId {
    let nt = NewTask {
        name: TaskName::parse(name).unwrap(),
        repo: "r".into(),
    };
    cx.store()
        .unwrap()
        .apply(Decision::insert(nt))
        .unwrap()
        .task
}

/// Give a task a Ready box acquired from `driver`.
fn ready_box(cx: &Cx, driver: &FakeDriver, id: TaskId) -> moor_core::BoxHandle {
    let p = ProviderId::new("p");
    let got = driver
        .acquire(&AcquireReq {
            provider: p.clone(),
            task: id,
            name: TaskName::parse("demo").unwrap(),
            egress: EgressPlan::Open,
            class: None,
            nonce: "abc123".into(),
            worktree: None,
        })
        .unwrap();
    let fields = Fields {
        box_provider: Field::Set(p),
        ..Fields::default()
    };
    let store = cx.store().unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let acquired = BoxInput::Acquired {
        handle: got.handle.clone(),
    };
    store.apply(Decision::new(id).box_input(acquired)).unwrap();
    got.handle
}

/// Box state of the only task, as the view reports it.
fn box_state(cx: &Cx) -> BoxState {
    let v = run(cx, &req(None, None)).unwrap();
    v.tasks.first().unwrap().r#box.as_ref().unwrap().state
}

#[test]
fn show_lists_all_tasks_with_no_selection() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    add(&cx, "a");
    add(&cx, "b");
    let v = run(&cx, &req(None, None)).unwrap();
    let names: Vec<_> = v.tasks.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, ["a", "b"]);
    assert!(v.task.is_none() && v.events.is_none());
}

#[test]
fn reconcile_marks_forgotten_box_gone() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    let handle = ready_box(&cx, &driver, id);
    assert_eq!(box_state(&cx), BoxState::Ready);
    driver.forget(&handle);
    assert_eq!(box_state(&cx), BoxState::Gone);
}

#[test]
fn list_failure_leaves_rows_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().fail_list(DriverError::Failed {
        summary: "boom".into(),
    }));
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    let handle = ready_box(&cx, &driver, id);
    driver.forget(&handle);
    assert_eq!(box_state(&cx), BoxState::Ready);
    let warned = cx.take_warnings();
    assert_eq!(warned.len(), 1);
    assert!(!warned.first().unwrap().message.contains("boom"));
}

#[test]
fn unsupported_list_is_skipped_silently() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new().fail_list(DriverError::Unsupported("box provider has no inventory")),
    );
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    let handle = ready_box(&cx, &driver, id);
    driver.forget(&handle);
    assert_eq!(box_state(&cx), BoxState::Ready);
    assert!(cx.take_warnings().is_empty());
}

#[test]
fn named_task_returns_last_events() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    cx.store()
        .unwrap()
        .apply(Decision::new(id).task_input(TaskInput::Admitted))
        .unwrap();
    let v = run(&cx, &req(Some("a"), Some(1))).unwrap();
    assert_eq!(v.task.unwrap().name, "a");
    assert_eq!(v.events.unwrap().len(), 1);
}

#[test]
fn unknown_task_name_fails() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let e = run(&cx, &req(Some("nope"), None)).unwrap_err();
    assert!(e.error.to_string().contains("no task nope"));
}

#[test]
fn task_shows_its_in_flight_run() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    let before = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(before.task.unwrap().run.is_none());
    cx.store()
        .unwrap()
        .begin_run(NewRun {
            task: id,
            argv: vec!["make".into(), "test".into()],
            record: None,
            sha: None,
            sync: "none".into(),
            pid: 4242,
        })
        .unwrap();
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    let r = v.task.unwrap().run.unwrap();
    assert_eq!(r.argv, ["make", "test"]);
    assert!(v.tasks.iter().all(|t| t.run.is_some()));
}

/// Context over `driver` with a scripted engine.
fn engine_cx(dir: &Path, driver: &Arc<FakeDriver>, engine: &Arc<FakeEngine>) -> Cx {
    Cx::fake(
        dir,
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new(dir)),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone())
}

/// Add a working task `name` with a box live in `driver`, and a live session
/// `session` when given.
fn working(cx: &Cx, driver: &FakeDriver, name: &str, session: Option<&str>) -> TaskId {
    let id = add(cx, name);
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: id,
            name: TaskName::parse(name).unwrap(),
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
    if let Some(s) = session {
        store.begin_session(id, s).unwrap();
    }
    id
}

/// Probes whose hook says `kind`, written just before the fake clock's now.
fn hook(cx: &Cx, kind: HookKind) -> Probes {
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind,
            session_id: None,
            at: Ts::from_millis(cx.clock().now().as_millis() - 1000),
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            exit_marker: None,
        }),
    }
}

#[test]
fn list_and_detail_report_the_same_agent() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "a", Some("s-a"));
    let engine = Arc::new(FakeEngine::new().probes("s-a", hook(&cx, HookKind::Idle)));
    let cx = engine_cx(dir.path(), &driver, &engine);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    let listed = v.tasks.first().unwrap().agent.clone().unwrap();
    let detail = v.task.unwrap().agent.unwrap();
    assert_eq!(listed, detail);
    assert_eq!(
        (detail.state.as_str(), detail.source.as_str()),
        ("idle", "hooks")
    );
}

#[test]
fn each_task_is_probed_once_per_command() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "a", Some("s-a"));
    working(&cx, &driver, "b", Some("s-b"));
    let engine = Arc::new(
        FakeEngine::new()
            .probes("s-a", hook(&cx, HookKind::Idle))
            .probes("s-b", hook(&cx, HookKind::Idle)),
    );
    let cx = engine_cx(dir.path(), &driver, &engine);
    run(&cx, &req(None, None)).unwrap();
    assert_eq!(engine.probe_count("s-a"), 1);
    assert_eq!(engine.probe_count("s-b"), 1);
    run(&cx, &req(Some("a"), Some(5))).unwrap();
    assert_eq!(engine.probe_count("s-a"), 2);
    assert_eq!(engine.probe_count("s-b"), 2);
}

#[test]
fn show_leaves_the_task_row_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", Some("s-a"));
    let engine = Arc::new(FakeEngine::new().probes("s-a", hook(&cx, HookKind::Idle)));
    let cx = engine_cx(dir.path(), &driver, &engine);
    let store = cx.store().unwrap();
    let before = store.task(id).unwrap();
    assert_eq!(before.as_ref().unwrap().state, TaskState::Working);
    run(&cx, &req(None, None)).unwrap();
    assert_eq!(store.task(id).unwrap(), before);
    run(&cx, &req(Some("a"), None)).unwrap();
    assert_eq!(store.task(id).unwrap(), before);
}

#[test]
fn task_without_a_live_session_has_no_agent() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "a", None);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(v.tasks.first().unwrap().agent.is_none());
    assert!(v.task.unwrap().agent.is_none());
}
