use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Tool;
use moor_core::seam::surface::Surface;
use moor_core::{ErrorKind, StartReq, TaskId, TaskName};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

mod activity;
mod capacity;
mod dotfiles;
mod proxy;
mod relaunch;
mod session;
mod setup;
mod transcript;
mod workdir;

/// A world of fakes plus the context over them.
struct World {
    cx: Cx,
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    proc: Arc<ScriptedProc>,
    dir: tempfile::TempDir,
}

/// Builds a world whose driver is `driver`.
fn world(driver: FakeDriver) -> World {
    world_at(driver, Path::new("/repo"))
}

/// Builds a world over the repo at `repo`.
fn world_at(driver: FakeDriver, repo: &Path) -> World {
    world_with(driver, repo, UserConfig::default())
}

/// Builds a world over the repo at `repo` with user config `config`.
fn world_with(driver: FakeDriver, repo: &Path, config: UserConfig) -> World {
    let dir = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["rev-parse", "--show-toplevel"],
        Reply::Ok(format!("{}\n", repo.display())),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok("abc123\n".into()),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok(format!("{}/.git\n", repo.display())),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--git-path", "info/exclude"],
        Reply::Ok(format!("{}/exclude\n", dir.path().display())),
    );
    proc.on_daemon(proxy::supervise(dir.path().to_path_buf()));
    let driver = Arc::new(driver);
    let surface = Arc::new(FakeSurface::new(dir.path()));
    let cx = Cx::fake_with(
        dir.path(),
        repo,
        driver.clone(),
        surface.clone(),
        proc.clone(),
        config,
    );
    World {
        cx,
        driver,
        surface,
        proc,
        dir,
    }
}

/// A start request for `name`.
fn req(name: &str) -> StartReq {
    StartReq {
        name: TaskName::parse(name).unwrap(),
        goal: None,
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
        resume: false,
    }
}

#[test]
fn happy_path_makes_a_working_task_with_a_ready_box() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!v.attached);
    assert_eq!(v.task.state, TaskState::Working);
    assert_eq!(v.task.base.as_deref(), Some("abc123"));
    assert_eq!(v.task.workspace.as_deref(), Some("ws-1"));
    assert_eq!(v.task.branch.as_deref(), Some("demo"));
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    let b = row.unwrap().unwrap().boxed.unwrap();
    assert_eq!(b.state, BoxState::Ready);
    assert_eq!(b.handle.unwrap().as_str(), "box-1");
    assert_eq!(b.workdir.as_deref(), Some("/work/cbx_1/demo"));
}

#[test]
fn happy_path_calls_seams_in_order_and_clears_intent() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    assert!(row.unwrap().unwrap().intent.is_none());
    assert_eq!(
        w.surface.calls(),
        ["create_worktree demo", "ensure_workspace moor:demo"]
    );
    assert_eq!(w.driver.calls(), ["acquire"]);
}

#[test]
fn start_excludes_scratch_dirs_from_the_repo() {
    let w = world(FakeDriver::new());
    run(&w.cx, &req("demo")).unwrap();
    assert_exclude_call(&w, Path::new("/repo"));
}

#[test]
fn start_excludes_scratch_dirs_when_a_worktree_is_bound() {
    let w = world(FakeDriver::new());
    let made = w
        .surface
        .create_worktree(Path::new("/repo"), &TaskName::parse("pre").unwrap(), None)
        .unwrap();
    let mut r = req("demo");
    r.worktree = Some(made.path.display().to_string());
    run(&w.cx, &r).unwrap();
    assert_exclude_call(&w, &made.path);
}

#[test]
fn exclude_failure_stops_start_before_any_worktree_exists() {
    let w = world(FakeDriver::new());
    break_exclude(&w);
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(w.surface.calls().is_empty(), "{:?}", w.surface.calls());
    let name = TaskName::parse("demo").unwrap();
    assert!(w
        .cx
        .store()
        .unwrap()
        .open_task_by_name(&name)
        .unwrap()
        .is_none());
}

#[test]
fn attach_excludes_scratch_dirs_again() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let wt = std::path::PathBuf::from(first.task.worktree.unwrap());
    let before = exclude_calls(&w);
    assert!(run(&w.cx, &req("demo")).unwrap().attached);
    assert_eq!(exclude_calls(&w), before + 1);
    assert_exclude_call(&w, &wt);
}

#[test]
fn attach_warns_when_the_exclude_fails_and_still_attaches() {
    let w = world(FakeDriver::new());
    run(&w.cx, &req("demo")).unwrap();
    break_exclude(&w);
    let again = run(&w.cx, &req("demo")).unwrap();
    assert!(again.attached);
    let warnings = w.cx.take_warnings();
    assert!(
        warnings.iter().any(|x| x.code == "exclude_failed"),
        "{warnings:?}"
    );
}

/// Makes the exclude file unreadable by putting a directory where it belongs.
fn break_exclude(w: &World) {
    let path = w.dir.path().join("exclude");
    let _ = std::fs::remove_file(&path);
    std::fs::create_dir(&path).unwrap();
}

/// How many times git was asked for `info/exclude`.
fn exclude_calls(w: &World) -> usize {
    w.proc
        .calls()
        .into_iter()
        .filter(|c| c.args == ["rev-parse", "--git-path", "info/exclude"])
        .count()
}

/// Asserts git was asked for `info/exclude` from inside `dir`.
fn assert_exclude_call(w: &World, dir: &Path) {
    let hit = w.proc.calls().into_iter().any(|c| {
        c.args == ["rev-parse", "--git-path", "info/exclude"] && c.cwd.as_deref() == Some(dir)
    });
    assert!(hit, "no exclude lookup in {}", dir.display());
}

#[test]
fn acquire_failure_keeps_a_failed_task_row() {
    let w = world(FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "boom".into(),
    }));
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    let shown = err.value.as_ref().expect("outcome rides the error");
    assert_eq!(
        shown.pointer("/task/state"),
        Some(&serde_json::json!("setup_failed"))
    );
    let name = TaskName::parse("demo").unwrap();
    let row =
        w.cx.store()
            .unwrap()
            .open_task_by_name(&name)
            .unwrap()
            .unwrap();
    assert_eq!(row.state, TaskState::SetupFailed);
    assert_eq!(row.boxed.unwrap().state, BoxState::Gone);
    assert!(row.intent.is_none());
    assert_eq!(w.driver.calls(), ["acquire", "release box-1"]);
}

#[test]
fn acquire_failure_with_release_error_keeps_failed_and_original_error() {
    let w = world(
        FakeDriver::new()
            .fail_acquire(DriverError::Failed {
                summary: "boom".into(),
            })
            .fail_release(DriverError::Failed {
                summary: "stop broke".into(),
            }),
    );
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert!(err.error.to_string().contains("boom"), "{err:?}");
    assert_eq!(
        err.value.as_ref().unwrap().pointer("/task/state"),
        Some(&serde_json::json!("setup_failed"))
    );
    let name = TaskName::parse("demo").unwrap();
    let row = w.cx.store().unwrap().open_task_by_name(&name).unwrap();
    let b = row.unwrap().boxed.unwrap();
    assert_eq!(b.state, BoxState::Failed);
    assert_eq!(b.attempted_handle.unwrap().as_str(), "box-1");
}

#[test]
fn restart_of_a_closed_name_creates_a_new_task() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("again")).unwrap();
    let id = TaskId::new(first.task.id);
    w.cx.store()
        .unwrap()
        .apply(
            Decision::new(id)
                .task_input(TaskInput::Close)
                .box_input(BoxInput::Released),
        )
        .unwrap();
    let second = run(&w.cx, &req("again")).unwrap();
    assert!(!second.attached);
    assert_ne!(second.task.id, first.task.id);
    assert_eq!(second.task.state, TaskState::Working);
}

#[test]
fn open_name_attaches_without_effects() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let again = run(&w.cx, &req("demo")).unwrap();
    assert!(again.attached);
    assert_eq!(again.task.id, first.task.id);
    assert_eq!(w.driver.calls(), ["acquire"]);
    assert_eq!(w.surface.calls().len(), 2);
}

#[test]
fn attach_writes_exactly_one_attached_event() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let count = |w: &World| {
        w.cx.store()
            .unwrap()
            .events(TaskId::new(first.task.id), 100)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "attached")
            .count()
    };
    assert_eq!(count(&w), 0);
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(count(&w), 1);
}

#[test]
fn open_name_with_another_worktree_is_refused() {
    let w = world(FakeDriver::new());
    run(&w.cx, &req("demo")).unwrap();
    let mut other = req("demo");
    other.worktree = Some("/elsewhere".into());
    let err = run(&w.cx, &other).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[test]
fn bound_worktree_skips_create_worktree() {
    let w = world(FakeDriver::new());
    let made = w
        .surface
        .create_worktree(Path::new("/repo"), &TaskName::parse("pre").unwrap(), None)
        .unwrap();
    let mut r = req("demo");
    r.worktree = Some(made.path.display().to_string());
    let v = run(&w.cx, &r).unwrap();
    let wt = made.path.display().to_string();
    assert_eq!(v.task.worktree.as_deref(), Some(wt.as_str()));
    assert_eq!(w.surface.calls().len(), 2);
    assert!(!w
        .surface
        .calls()
        .iter()
        .any(|c| c == "create_worktree demo"));
    assert_eq!(v.task.state, TaskState::Working);
}

#[test]
fn start_after_acquire_failure_gets_a_new_box() {
    let w = world(FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "boom".into(),
    }));
    run(&w.cx, &req("demo")).unwrap_err();
    let again = run(&w.cx, &req("demo")).unwrap();
    assert!(again.attached);
    assert_eq!(again.task.state, TaskState::Working);
    let row = w.cx.store().unwrap().task(TaskId::new(again.task.id));
    assert_eq!(row.unwrap().unwrap().boxed.unwrap().state, BoxState::Ready);
    assert_eq!(w.driver.calls(), ["acquire", "release box-1", "acquire"]);
    assert_eq!(w.surface.calls().len(), 2);
}

#[test]
fn start_on_a_gone_box_acquires_again_and_keeps_the_task_working() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let again = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(again.task.state, TaskState::Working);
    let b = store.task(id).unwrap().unwrap().boxed.unwrap();
    assert_eq!(b.state, BoxState::Ready);
    assert_ne!(b.handle.unwrap().as_str(), "box-1");
    assert_eq!(w.driver.calls(), ["acquire", "acquire"]);
}

#[test]
fn main_worktree_is_refused_before_any_effect() {
    let w = world(FakeDriver::new());
    let mut r = req("demo");
    r.worktree = Some("/repo".into());
    let err = run(&w.cx, &r).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(w.surface.calls().is_empty());
    assert!(w.driver.calls().is_empty());
}

#[test]
fn nonces_at_the_same_clock_instant_differ_and_come_from_the_rng() {
    let w = world(FakeDriver::new());
    let at = w.cx.clock().now();
    let a = crate::ops::common::provision::nonce(w.cx.rng());
    let b = crate::ops::common::provision::nonce(w.cx.rng());
    assert_eq!(w.cx.clock().now(), at);
    assert_ne!(a, b);
    assert_eq!(a.len(), 6);
    assert!(a
        .bytes()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

#[test]
fn unresolved_credential_rejects_before_any_side_effect() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join(".moor.yaml"),
        "box_credentials: [MISSING]\n",
    )
    .unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("MISSING"), "{err:?}");
    assert_eq!(
        err.value.as_ref().unwrap().pointer("/task/state"),
        Some(&serde_json::json!("rejected"))
    );
    assert!(w.surface.calls().is_empty());
    assert!(w.driver.calls().is_empty());
    let name = TaskName::parse("demo").unwrap();
    assert!(w
        .cx
        .store()
        .unwrap()
        .open_task_by_name(&name)
        .unwrap()
        .is_none());
}

#[test]
fn start_refuses_while_another_op_holds_the_box_lock() {
    let w = world(FakeDriver::new());
    let _held = BoxLock::acquire(
        w.cx.clock(),
        w.cx.state_dir().unwrap(),
        TaskId::new(1),
        "close",
        Duration::ZERO,
    )
    .unwrap();
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), "box busy: close in progress");
    assert!(w.driver.calls().is_empty());
}

/// Holds the Box lock of task 1 as `op`.
fn hold(w: &World, op: &str) -> BoxLock {
    BoxLock::acquire(
        w.cx.clock(),
        w.cx.state_dir().unwrap(),
        TaskId::new(1),
        op,
        Duration::ZERO,
    )
    .unwrap()
}

#[test]
fn refused_start_provisions_when_retried_after_the_lock_is_released() {
    let w = world(FakeDriver::new());
    let held = hold(&w, "close");
    run(&w.cx, &req("demo")).unwrap_err();
    drop(held);
    let again = run(&w.cx, &req("demo")).unwrap();
    assert!(again.attached);
    assert_eq!(again.task.state, TaskState::Working);
    let row = w.cx.store().unwrap().task(TaskId::new(again.task.id));
    assert_eq!(row.unwrap().unwrap().boxed.unwrap().state, BoxState::Ready);
    assert_eq!(w.driver.calls(), ["acquire"]);
}

#[test]
fn start_on_a_lost_box_is_refused_while_another_op_holds_the_lock() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let _held = hold(&w, "close");
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), "box busy: close in progress");
    assert_eq!(w.driver.calls(), ["acquire"]);
}

#[test]
fn reprovision_skips_when_another_start_already_gave_the_box_back() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let stale = store.task(id).unwrap().unwrap();
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(w.driver.calls(), ["acquire", "acquire"]);
    let run = crate::ops::common::provision::reprovision(&w.cx, store, &stale, "start").unwrap();
    assert!(run.is_none());
    assert_eq!(w.driver.calls(), ["acquire", "acquire"]);
}

/// Starts `demo`, then leaves it as a provisioning that lost its Box handle.
fn stuck_provisioning(w: &World) {
    let first = run(&w.cx, &req("demo")).unwrap();
    let stuck = Decision::new(TaskId::new(first.task.id))
        .set(Fields {
            box_handle: Field::Clear,
            ..Fields::default()
        })
        .intent(IntentChange::Set(Intent::Provisioning { pid: 4242 }));
    w.cx.store().unwrap().apply(stuck).unwrap();
}

#[test]
fn dead_provisioning_is_refused_with_a_close_hint() {
    let w = world(FakeDriver::new());
    stuck_provisioning(&w);
    w.proc.dead(4242);
    w.cx.clock().sleep(Duration::from_secs(121));
    let before = w.driver.calls().len();
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(
        err.error.to_string(),
        "provisioning dead; run moor close demo"
    );
    assert!(err.hints.iter().any(|h| h == "moor close demo"), "{err:?}");
    assert_eq!(w.driver.calls().len(), before);
}

#[test]
fn live_provisioning_pid_attaches() {
    let w = world(FakeDriver::new());
    stuck_provisioning(&w);
    w.cx.clock().sleep(Duration::from_secs(121));
    assert!(run(&w.cx, &req("demo")).unwrap().attached);
}

#[test]
fn dead_provisioning_within_the_grace_period_attaches() {
    for secs in [119, 120] {
        let w = world(FakeDriver::new());
        stuck_provisioning(&w);
        w.proc.dead(4242);
        w.cx.clock().sleep(Duration::from_secs(secs));
        assert!(run(&w.cx, &req("demo")).unwrap().attached, "{secs}s");
    }
}
