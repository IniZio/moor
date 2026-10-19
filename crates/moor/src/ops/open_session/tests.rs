//! Tests for `open-session`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::session_state::SessionState;
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::request::OpenSessionReq;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::engine::{EngineError, LaunchPrompt};
use moor_core::{BoxId, BoxName, ErrorKind, ProviderId, SessionId};

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    engine: Arc<FakeEngine>,
    id: BoxId,
}

/// Task `t` with a ready Box whose first Session ended, or is still live when `live`.
fn fx(engine: FakeEngine, live: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let engine = Arc::new(engine);
    let surface = Arc::new(FakeSurface::new(PathBuf::from("/s")));
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        moor_core::seam::proc::Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/home/me/proj/.git\n".into()),
    );
    let cx = Cx::fake(
        dir.path(),
        Path::new("/home/me/proj"),
        driver.clone(),
        surface,
        proc,
    )
    .with_engine(engine.clone());
    let id = ready_task(&cx, &driver);
    let store = cx.store().unwrap();
    store.begin_session(id, "s-1", Some("p-0")).unwrap();
    if !live {
        assert!(store.end_current_session(id, "requested", None).unwrap());
    }
    Fx {
        _dir: dir,
        cx,
        engine,
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
        workspace_id: Field::Set("w-1".to_owned()),
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

fn req() -> OpenSessionReq {
    OpenSessionReq {
        task: "t".into(),
        seed: "pick up here".into(),
        parent: None,
    }
}

fn kinds(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter().map(|e| e.kind).collect()
}

#[test]
fn a_ready_box_opens_the_next_session_seeded_with_the_text() {
    let f = fx(FakeEngine::new(), false);
    let store = f.cx.store().unwrap();
    let v = run(&f.cx, &req()).unwrap();
    let sessions = store.box_sessions(f.id).unwrap();
    assert_eq!(sessions.len(), 2);
    let next = sessions.get(1).unwrap();
    assert_eq!(
        (v.task.as_str(), v.session, v.ordinal, v.pane.as_deref()),
        ("t", next.id, 2, Some("p-1"))
    );
    assert_eq!(next.state, SessionState::Working);
    let launches = f.engine.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(
        launches.first().unwrap().prompt,
        LaunchPrompt::Text("pick up here".into())
    );
    assert_eq!(
        store.live_session(f.id).unwrap().unwrap().pane.as_deref(),
        Some("p-1")
    );
}

#[test]
fn a_live_session_is_refused_and_opens_nothing() {
    let f = fx(FakeEngine::new(), true);
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), "Box t already has a live Session");
    assert_eq!(f.cx.store().unwrap().box_sessions(f.id).unwrap().len(), 1);
    assert!(f.engine.launches().is_empty());
}

#[test]
fn an_asleep_box_is_refused_and_opens_nothing() {
    let f = fx(FakeEngine::new(), false);
    let store = f.cx.store().unwrap();
    let quiet = Decision::new(f.id).task_input(TaskInput::AgentQuiet);
    store.apply(quiet).unwrap();
    let d = Decision::new(f.id)
        .task_input(TaskInput::SleepCmd)
        .box_input(BoxInput::Sleep);
    store.apply(d).unwrap();
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.hints.iter().any(|h| h.contains("moor wake t")));
    assert_eq!(store.box_sessions(f.id).unwrap().len(), 1);
    assert!(f.engine.launches().is_empty());
}

#[test]
fn a_failed_launch_records_the_failure_and_ends_the_new_session() {
    let engine = FakeEngine::new().launch_result(Err(EngineError::Failed {
        step: "launch",
        message: "no pane".into(),
    }));
    let f = fx(engine, false);
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("no pane"));
    let sessions = f.cx.store().unwrap().box_sessions(f.id).unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions.get(1).unwrap().state, SessionState::Ended);
    assert_eq!(
        kinds(&f).iter().filter(|k| *k == "session_failed").count(),
        1
    );
}

#[test]
fn an_unknown_task_is_not_found() {
    let f = fx(FakeEngine::new(), false);
    let err = run(
        &f.cx,
        &OpenSessionReq {
            task: "x".into(),
            seed: String::new(),
            parent: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
}

/// The `parent_session` column of Session `id`.
fn parent_of(f: &Fx, id: i64) -> Option<i64> {
    let raw = rusqlite::Connection::open(f.cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.query_row(
        "SELECT parent_session FROM sessions WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn a_live_parent_is_recorded_on_the_new_session() {
    let f = fx(FakeEngine::new(), false);
    let parent =
        f.cx.store()
            .unwrap()
            .open_host_session("/w", None, None)
            .unwrap();
    let v = run(
        &f.cx,
        &OpenSessionReq {
            parent: Some(SessionId::new(parent)),
            ..req()
        },
    )
    .unwrap();
    assert_eq!(parent_of(&f, v.session), Some(parent));
}

#[test]
fn no_parent_is_recorded_when_none_is_given() {
    let f = fx(FakeEngine::new(), false);
    let v = run(&f.cx, &req()).unwrap();
    assert_eq!(parent_of(&f, v.session), None);
}

#[test]
fn an_unknown_parent_is_refused_and_opens_nothing() {
    let f = fx(FakeEngine::new(), false);
    let err = run(
        &f.cx,
        &OpenSessionReq {
            parent: Some(SessionId::new(9999)),
            ..req()
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("no Session 9999"));
    assert_eq!(f.cx.store().unwrap().box_sessions(f.id).unwrap().len(), 1);
    assert!(f.engine.launches().is_empty());
}

#[test]
fn an_ended_parent_is_refused_and_opens_nothing() {
    let f = fx(FakeEngine::new(), false);
    let store = f.cx.store().unwrap();
    let parent = store.open_host_session("/w", None, None).unwrap();
    store.end_host_session(parent).unwrap();
    let err = run(
        &f.cx,
        &OpenSessionReq {
            parent: Some(SessionId::new(parent)),
            ..req()
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(store.box_sessions(f.id).unwrap().len(), 1);
    assert!(f.engine.launches().is_empty());
}
