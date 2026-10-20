//! Tests for `open-session`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::activity::{PaneObs, PaneStatus};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::{SessionInput, SessionState};
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
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    id: BoxId,
}

/// Box `t` with a ready machine whose first Session ended, or is still live when `live`.
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
        surface.clone(),
        proc,
    )
    .with_engine(engine.clone());
    let id = ready_box(&cx, &driver);
    let store = cx.store().unwrap();
    store.begin_session(id, "s-1", Some("p-0")).unwrap();
    if !live {
        assert!(store.end_current_session(id, "requested", None).unwrap());
    }
    Fx {
        _dir: dir,
        cx,
        engine,
        driver,
        surface,
        id,
    }
}

/// Box `t` with a ready machine.
fn ready_box(cx: &Cx, driver: &FakeDriver) -> BoxId {
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set("/home/me/wt/t".to_owned()),
        workspace_id: Field::Set("w-1".to_owned()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
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
        box_name: "t".into(),
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
        (v.box_name.as_str(), v.session, v.ordinal, v.pane.as_deref()),
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
fn an_open_makes_no_box_call() {
    let f = fx(FakeEngine::new(), false);
    let before = f.driver.calls();
    run(&f.cx, &req()).unwrap();
    assert_eq!(f.engine.launches().len(), 1);
    assert_eq!(f.driver.calls(), before);
}

#[test]
fn an_agent_stopped_by_an_unwritable_proxy_ca_fails_the_open_with_the_ca_message() {
    let f = fx(FakeEngine::new(), false);
    let ca = "the Proxy CA could not be written to /home/u/.moor-proxy/ca.pem";
    let obs = PaneObs {
        status: PaneStatus::Unknown,
        tail: format!("moor: {ca}\nmoor: agent exited 1"),
        prompt: None,
        exit_marker: Some(1),
    };
    f.surface.push_pane_obs("p-1", Ok(obs));
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(err.error.to_string(), format!("launch: {ca}"));
    let store = f.cx.store().unwrap();
    assert!(store.live_session(f.id).unwrap().is_none());
    let row = store.box_row(f.id).unwrap().unwrap();
    assert_eq!(row.launch_failure.map(|l| l.message).as_deref(), Some(ca));
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
    let quiet = Decision::new(f.id).session(SessionInput::AgentQuiet);
    store.apply(quiet).unwrap();
    let d = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Sleep { auto: false })
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
fn an_unknown_box_is_not_found() {
    let f = fx(FakeEngine::new(), false);
    let err = run(
        &f.cx,
        &OpenSessionReq {
            box_name: "x".into(),
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
