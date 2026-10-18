//! Tests for waking an Asleep Task and Box.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, SleepOutcome};
use moor_core::seam::engine::SessionRef;
use moor_core::seam::proc::Tool;
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ErrorKind, ProviderId};

use super::{wake_and_revive, wake_box_if_asleep, wake_with_tunnel};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{Store, TaskRow};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

mod tunnel;

/// Wake and restore the tunnel, without relaunching the agent.
fn wake_if_asleep(cx: &Cx, store: &Store, row: &TaskRow) -> Result<bool, CommandError> {
    wake_with_tunnel(cx, store, row).map(|w| w.any)
}

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    engine: Arc<FakeEngine>,
    id: BoxId,
}

/// A launched session whose pane is `pane`.
fn session(pane: &str) -> SessionRef {
    SessionRef {
        id: String::new(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: PathBuf::from("/home/me/wt/t"),
        pane: Some(PaneRef::new(pane)),
        warnings: Vec::new(),
    }
}

fn acquire_req(provider: ProviderId, task: BoxId, name: BoxName) -> AcquireReq {
    AcquireReq {
        provider,
        task,
        name,
        class: None,
        nonce: "n".to_owned(),
        worktree: Some(PathBuf::from("/home/me/wt/t")),
    }
}

/// A process runner that answers the git lookups a relaunch makes.
fn git_proc() -> Arc<ScriptedProc> {
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/home/me/proj/.git\n".into()),
    );
    proc
}

/// Task `t` Working with a Ready Box.
fn fx_with(driver: FakeDriver) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let surface = Arc::new(FakeSurface::new(PathBuf::from("/s")));
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(session("p-2"))));
    let proc = git_proc();
    let cx = Cx::fake(
        dir.path(),
        Path::new("/home/me/proj"),
        driver.clone(),
        surface.clone(),
        proc,
    )
    .with_engine(engine.clone());
    let store = cx.store().unwrap();
    let name = BoxName::parse("t").unwrap();
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
    store
        .apply(
            Decision::new(id)
                .task_input(TaskInput::Admitted)
                .box_input(BoxInput::Requested)
                .set(fields),
        )
        .unwrap();
    let got = driver.acquire(&acquire_req(provider, id, name)).unwrap();
    store
        .apply(
            Decision::new(id)
                .task_input(TaskInput::BoxReady {
                    setup: SetupOutcome::Ok,
                })
                .box_input(BoxInput::Acquired { handle: got.handle })
                .set(Fields {
                    box_workdir: Field::Set(got.workdir.as_str().to_owned()),
                    ..Fields::default()
                }),
        )
        .unwrap();
    Fx {
        _dir: dir,
        cx,
        driver,
        surface,
        engine,
        id,
    }
}

fn fx() -> Fx {
    fx_with(FakeDriver::new())
}

/// Put the Task and Box to sleep.
fn sleep(f: &Fx) {
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    store
        .apply(
            Decision::new(f.id)
                .task_input(TaskInput::SleepCmd)
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
}

fn row(f: &Fx) -> TaskRow {
    let name = BoxName::parse("t").unwrap();
    f.cx.store()
        .unwrap()
        .open_task_by_name(&name)
        .unwrap()
        .unwrap()
}

fn kinds(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter().map(|e| e.kind).collect()
}

fn workdir_calls(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("workdir"))
        .count()
}

fn stored_workdir(f: &Fx) -> Option<String> {
    row(f).boxed.unwrap().workdir
}

fn wake_calls(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("wake"))
        .count()
}

#[test]
fn asleep_task_and_box_are_woken() {
    let f = fx();
    sleep(&f);
    let store = f.cx.store().unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert!(f.driver.calls().contains(&"wake box-1".to_owned()));
    assert_eq!(wake_calls(&f), 1);
    let r = row(&f);
    assert_eq!(r.state, TaskState::Working);
    assert_eq!(r.boxed.unwrap().state, BoxState::Ready);
    assert!(kinds(&f).contains(&"woke".to_owned()));
}

#[test]
fn awake_task_is_left_alone() {
    let f = fx();
    let store = f.cx.store().unwrap();
    assert!(!wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(wake_calls(&f), 0);
    assert_eq!(row(&f).state, TaskState::Working);
}

#[test]
fn failed_resume_leaves_task_and_box_asleep() {
    let f = fx_with(FakeDriver::new().fail_wake(DriverError::NotFound));
    sleep(&f);
    let store = f.cx.store().unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).is_err());
    let r = row(&f);
    assert_eq!(r.state, TaskState::Asleep);
    assert_eq!(r.boxed.unwrap().state, BoxState::Asleep);
    assert!(!kinds(&f).contains(&"woke".to_owned()));
}

#[test]
fn gone_box_is_refused_without_a_driver_call() {
    let f = fx();
    sleep(&f);
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let err = wake_if_asleep(&f.cx, store, &row(&f)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(wake_calls(&f), 0);
}

#[test]
fn asleep_task_with_ready_box_wakes_only_the_task() {
    let f = fx();
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::SleepCmd))
        .unwrap();
    assert_eq!(row(&f).state, TaskState::Asleep);
    let ready_before = kinds(&f).iter().filter(|k| *k == "box_ready").count();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(wake_calls(&f), 0);
    let r = row(&f);
    assert_eq!(r.state, TaskState::Working);
    assert_eq!(r.boxed.unwrap().state, BoxState::Ready);
    let after = kinds(&f);
    assert!(after.contains(&"woke".to_owned()));
    assert_eq!(
        after.iter().filter(|k| *k == "box_ready").count(),
        ready_before
    );
}

#[test]
fn asleep_box_with_working_task_wakes_only_the_box() {
    let f = fx();
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).box_input(BoxInput::Sleep))
        .unwrap();
    assert_eq!(row(&f).boxed.unwrap().state, BoxState::Asleep);
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(wake_calls(&f), 1);
    let r = row(&f);
    assert_eq!(r.state, TaskState::Working);
    assert_eq!(r.boxed.unwrap().state, BoxState::Ready);
    assert!(!kinds(&f).contains(&"woke".to_owned()));
}

#[test]
fn waking_a_box_with_a_stored_workdir_does_not_probe() {
    let f = fx();
    sleep(&f);
    f.driver.set_work_root("/moved");
    let store = f.cx.store().unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(f.driver.box_calls(), ["acquire", "wake"]);
    assert_eq!(workdir_calls(&f), 0);
    assert_eq!(stored_workdir(&f).as_deref(), Some("/work/cbx_1/t"));
}

fn clear_workdir(f: &Fx) {
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(Fields {
            box_workdir: Field::Clear,
            ..Fields::default()
        }))
        .unwrap();
    assert_eq!(stored_workdir(f), None);
}

#[test]
fn waking_a_box_without_a_stored_workdir_probes_once_and_stores_it() {
    let f = fx();
    sleep(&f);
    clear_workdir(&f);
    f.driver.set_work_root("/moved");
    let store = f.cx.store().unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(f.driver.box_calls(), ["acquire", "wake", "workdir"]);
    assert_eq!(stored_workdir(&f).as_deref(), Some("/moved/cbx_1/t"));
}

#[test]
fn failed_workdir_lookup_after_wake_leaves_task_and_box_asleep() {
    let f = fx();
    sleep(&f);
    clear_workdir(&f);
    f.driver.fail_workdir(DriverError::NotFound);
    let store = f.cx.store().unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).is_err());
    let r = row(&f);
    assert_eq!(r.state, TaskState::Asleep);
    assert_eq!(r.boxed.unwrap().state, BoxState::Asleep);
    assert_eq!(stored_workdir(&f), None);
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(workdir_calls(&f), 2);
}

#[test]
fn waking_only_the_task_does_not_look_the_workdir_up() {
    let f = fx();
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::SleepCmd))
        .unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(workdir_calls(&f), 0);
}

/// Gives the Task a live agent session in pane `p-1` that can be resumed.
fn with_agent(f: &Fx) {
    let store = f.cx.store().unwrap();
    store.begin_session(f.id, "agent-1", Some("p-1")).unwrap();
    let set = Fields {
        agent_session_id: Field::Set("agent-1".into()),
        workspace_id: Field::Set("ws-1".into()),
        ..Fields::default()
    };
    store.apply(Decision::new(f.id).set(set)).unwrap();
}

fn relaunched(f: &Fx) -> Vec<serde_json::Value> {
    f.cx.store()
        .unwrap()
        .events(f.id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "relaunched")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn waking_a_box_whose_agent_is_gone_relaunches_it() {
    let f = fx();
    with_agent(&f);
    sleep(&f);
    f.surface.push_pane_obs("p-1", Err(SurfaceError::NotFound));
    let store = f.cx.store().unwrap();
    assert!(wake_and_revive(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(f.engine.launches().len(), 1);
    let events = relaunched(&f);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().and_then(|e| e.get("cause")),
        Some(&serde_json::json!("wake"))
    );
}

#[test]
fn waking_a_box_whose_agent_is_live_does_not_relaunch() {
    let f = fx();
    with_agent(&f);
    sleep(&f);
    let store = f.cx.store().unwrap();
    assert!(wake_and_revive(&f.cx, store, &row(&f)).unwrap());
    assert!(f.engine.launches().is_empty());
    assert!(relaunched(&f).is_empty());
}

#[test]
fn waking_only_the_task_does_not_probe_the_agent() {
    let f = fx();
    with_agent(&f);
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::SleepCmd))
        .unwrap();
    f.surface.push_pane_obs("p-1", Err(SurfaceError::NotFound));
    assert!(wake_and_revive(&f.cx, store, &row(&f)).unwrap());
    assert!(f.engine.launches().is_empty());
    assert!(relaunched(&f).is_empty());
}

#[test]
fn plain_wake_leaves_a_gone_agent_alone() {
    let f = fx();
    with_agent(&f);
    sleep(&f);
    f.surface.push_pane_obs("p-1", Err(SurfaceError::NotFound));
    let store = f.cx.store().unwrap();
    assert!(wake_box_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert!(f.engine.launches().is_empty());
}

#[test]
fn a_pane_closed_by_sleep_is_relaunched_on_wake() {
    let f = fx();
    with_agent(&f);
    f.surface.have_pane("p-1");
    crate::ops::sleep::run(
        &f.cx,
        &moor_core::TaskReq {
            task: BoxName::parse("t").unwrap(),
        },
    )
    .unwrap();
    assert!(f.surface.calls().contains(&"close_pane p-1".to_owned()));
    f.surface.push_pane_obs("p-1", Err(SurfaceError::NotFound));
    let store = f.cx.store().unwrap();
    assert!(wake_and_revive(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(f.engine.launches().len(), 1);
    assert_eq!(relaunched(&f).len(), 1);
}

#[test]
fn a_box_that_cannot_sleep_gets_its_agent_back() {
    let f = fx_with(FakeDriver::new().sleep_outcome(SleepOutcome::CannotSleep("no".into())));
    with_agent(&f);
    f.surface.have_pane("p-1");
    f.surface.push_pane_obs("p-1", Err(SurfaceError::NotFound));
    let req = moor_core::TaskReq {
        task: BoxName::parse("t").unwrap(),
    };
    assert!(crate::ops::sleep::run(&f.cx, &req).is_err());
    assert!(f.surface.calls().contains(&"close_pane p-1".to_owned()));
    assert_eq!(f.engine.launches().len(), 1);
    assert_eq!(relaunched(&f).len(), 1);
}
