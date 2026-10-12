use std::path::Path;
use std::sync::Arc;

use moor_core::config::{parse_user, UserConfig};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::Decision;
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Tool;
use moor_core::seam::surface::Surface;
use moor_core::{ErrorKind, StartReq, TaskId, TaskName};

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

/// A world of fakes plus the context over them.
struct World {
    cx: Cx,
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    proc: Arc<ScriptedProc>,
    _dir: tempfile::TempDir,
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
        _dir: dir,
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
    assert_eq!(b.workdir.as_deref(), Some("/work/box-1"));
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
    let a = super::provision::nonce(w.cx.rng());
    let b = super::provision::nonce(w.cx.rng());
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
fn identity_from_user_config_is_set_on_the_worktree() {
    let repo = tempfile::tempdir().unwrap();
    let user = parse_user("t", "git:\n  name: User Name\n  email: r@example.com\n").unwrap();
    let w = world_with(FakeDriver::new(), repo.path(), user);
    w.proc.on(Tool::Git, &["config"], Reply::Ok(String::new()));
    run(&w.cx, &req("demo")).unwrap();
    let configs: Vec<Vec<String>> = w
        .proc
        .calls()
        .into_iter()
        .map(|c| c.args)
        .filter(|a| a.first().map(String::as_str) == Some("config"))
        .collect();
    assert_eq!(
        configs.last().unwrap(),
        &["config", "--worktree", "user.email", "r@example.com"]
    );
    assert_eq!(configs.len(), 3);
}
