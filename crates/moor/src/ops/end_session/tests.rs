//! Tests for `end-session`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::session_state::SessionState;
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::request::EndSessionReq;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::proc::Tool;
use moor_core::{BoxId, BoxName, ErrorKind, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    id: BoxId,
}

/// Task `t` with a ready Box and, when `session`, a live launch in pane `p-1` that registered an
/// agent session.
fn fx_with(session: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let engine = Arc::new(FakeEngine::new().transcript_text("abc"));
    let surface = Arc::new(FakeSurface::new(PathBuf::from("/s")));
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/home/me/proj/.git\n".into()),
    );
    let cx = Cx::fake(
        dir.path(),
        Path::new("/home/me/proj"),
        driver.clone(),
        surface.clone(),
        proc,
    )
    .with_engine(engine);
    let id = ready_task(&cx, &driver);
    if session {
        let store = cx.store().unwrap();
        let launch = store.begin_session(id, "s-1", Some("p-1")).unwrap();
        store.set_registered_session(launch, "agent-1").unwrap();
        surface.have_pane("p-1");
    }
    Fx {
        _dir: dir,
        cx,
        driver,
        surface,
        id,
    }
}

/// Task `t` with a ready Box.
fn ready_task(cx: &Cx, driver: &FakeDriver) -> BoxId {
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set("/home/me/wt/t".to_owned()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            provider,
            task: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired {
            handle: got.handle.clone(),
        })
        .set(Fields {
            box_handle: Field::Set(got.handle),
            box_workdir: Field::Set(got.workdir.as_str().to_owned()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    id
}

fn req() -> EndSessionReq {
    EndSessionReq { task: "t".into() }
}

fn kinds(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter().map(|e| e.kind).collect()
}

fn count(f: &Fx, kind: &str) -> usize {
    kinds(f).iter().filter(|k| *k == kind).count()
}

#[test]
fn a_live_session_ends_and_the_box_stays_ready() {
    let f = fx_with(true);
    let store = f.cx.store().unwrap();
    let v = run(&f.cx, &req()).unwrap();
    let sessions = store.box_sessions(f.id).unwrap();
    let first = sessions.first().unwrap();
    assert_eq!(
        (v.task.as_str(), v.session, v.ordinal, v.box_kept),
        ("t", first.id, first.ordinal, true)
    );
    assert_eq!(first.state, SessionState::Ended);
    let order = kinds(&f);
    let saved = order.iter().position(|k| k == "transcript_saved").unwrap();
    let ended = order.iter().position(|k| k == "session_ended").unwrap();
    assert!(saved < ended, "{order:?}");
    let ended_event = store
        .events(f.id, 100)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "session_ended")
        .unwrap();
    assert_eq!(
        ended_event.payload.get("cause").and_then(|c| c.as_str()),
        Some("requested")
    );
    assert!(f.surface.calls().iter().any(|c| c == "close_pane p-1"));
    assert!(store.live_session(f.id).unwrap().is_none());
    let row = store.open_task_by_name(&BoxName::parse_allow_bare("t").unwrap());
    let row = row.unwrap().unwrap();
    assert_eq!(row.boxed.unwrap().state, BoxState::Ready);
    assert_eq!(row.state, TaskState::Idle);
}

#[test]
fn ending_twice_refuses_the_second_time_without_a_new_event() {
    let f = fx_with(true);
    run(&f.cx, &req()).unwrap();
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), "no live Session in Box t");
    assert_eq!(count(&f, "session_ended"), 1);
}

#[test]
fn a_launch_that_already_stopped_is_refused() {
    let f = fx_with(true);
    let store = f.cx.store().unwrap();
    let launch = store.live_session(f.id).unwrap().unwrap();
    store.end_session(launch.id, None, None).unwrap();
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(count(&f, "session_ended"), 0);
}

#[test]
fn an_unknown_task_is_not_found() {
    let f = fx_with(true);
    let err = run(&f.cx, &EndSessionReq { task: "x".into() }).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("no open task x"));
}

#[test]
fn an_asleep_box_wakes_before_the_session_ends() {
    let f = fx_with(true);
    let store = f.cx.store().unwrap();
    let quiet = Decision::new(f.id).task_input(TaskInput::AgentQuiet);
    store.apply(quiet).unwrap();
    let d = Decision::new(f.id)
        .task_input(TaskInput::SleepCmd)
        .box_input(BoxInput::Sleep);
    store.apply(d).unwrap();
    run(&f.cx, &req()).unwrap();
    let row = store.task(f.id).unwrap().unwrap();
    assert_ne!(row.state, TaskState::Asleep);
    assert_eq!(row.boxed.unwrap().state, BoxState::Ready);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
    assert_eq!(count(&f, "session_ended"), 1);
}

#[test]
fn a_pane_that_will_not_close_keeps_the_session_live() {
    let f = fx_with(true);
    f.surface
        .set_fail_close_pane(moor_core::seam::surface::SurfaceError::Failed {
            summary: "boom".into(),
        });
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("the Session stays live"));
    assert_eq!(count(&f, "session_ended"), 0);
}
