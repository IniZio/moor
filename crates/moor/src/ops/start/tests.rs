use std::{path::Path, sync::Arc, time::Duration};

use moor_core::config::UserConfig;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Tool;
use moor_core::{BoxId, BoxPath, ErrorKind, StartReq};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;
use crate::testing::{box_name, driver::FakeDriver};

mod activity;
mod base;
mod branch_owner;
mod capacity;
mod dead_agent;
mod dotfiles;
mod exclude;
mod exited_start;
mod hold;
mod identity;
mod known_repo;
mod launch_skipped;
mod launch_warnings;
mod lease;
mod lease_dirs;
mod lease_handle;
mod leftover;
mod moor_yaml;
mod others;
mod parent;
mod provider_caps;
mod proxy;
mod relaunch;
mod relaunch_sessions;
mod relaunch_yaml;
mod requester;
mod rerun;
mod revive;
mod session;
mod settle_busy;
mod setup;
mod setup_rerun;
mod state;
mod sync_proxy;
mod sync_retry;
mod sync_timing;
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
    proc.on(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
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
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let driver = Arc::new((1..=4).fold(driver, |d, n| {
        d.peek_text(&format!("box-{n}"), &environ, "HOME=/home/box\0")
    }));
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
        name: box_name(name),
        goal: None,
        issue: None,
        base: None,
        model: None,
        no_agent: false,
        hold: false,
        resume: false,
        trust_moor_yaml: false,
        repo: None,
        parent: None,
        identity: None,
    }
}

#[test]
fn happy_path_makes_a_working_box_with_a_ready_box() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!v.attached);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), state::WORKING);
    assert_eq!(v.boxed.base.as_deref(), Some("abc123"));
    assert_eq!(v.boxed.workspace.as_deref(), Some("ws-1"));
    assert_eq!(v.boxed.branch.as_deref(), Some("demo"));
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
    let b = row.unwrap().unwrap().boxed.unwrap();
    assert_eq!(b.state, BoxState::Ready);
    assert_eq!(b.handle.unwrap().as_str(), "box-1");
    assert_eq!(b.workdir.as_deref(), Some("/work/cbx_1/demo"));
}

#[test]
fn happy_path_calls_seams_in_order_and_clears_intent() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
    assert!(row.unwrap().unwrap().intent.is_none());
    assert_eq!(
        w.surface.calls(),
        ["create_worktree demo", "ensure_workspace moor:demo"]
    );
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1"]);
}

#[test]
fn acquire_failure_keeps_a_failed_box_row() {
    let w = world(FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "boom".into(),
    }));
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    let shown = err.value.as_ref().expect("outcome rides the error");
    assert_eq!(
        shown.pointer("/box/lifecycle"),
        Some(&serde_json::json!("setup_failed"))
    );
    let name = box_name("demo");
    let row =
        w.cx.store()
            .unwrap()
            .open_box_by_name(&name)
            .unwrap()
            .unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
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
        err.value.as_ref().unwrap().pointer("/box/lifecycle"),
        Some(&serde_json::json!("setup_failed"))
    );
    let name = box_name("demo");
    let row = w.cx.store().unwrap().open_box_by_name(&name).unwrap();
    let b = row.unwrap().boxed.unwrap();
    assert_eq!(b.state, BoxState::Failed);
    assert_eq!(b.attempted_handle.unwrap().as_str(), "box-1");
}

#[test]
fn restart_of_a_closed_name_creates_a_new_box() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("again")).unwrap();
    let id = BoxId::new(first.boxed.id);
    w.cx.store()
        .unwrap()
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End)
                .box_input(BoxInput::Released),
        )
        .unwrap();
    let second = run(&w.cx, &req("again")).unwrap();
    assert!(!second.attached);
    assert_ne!(second.boxed.id, first.boxed.id);
    assert_eq!(
        (second.boxed.lifecycle, second.boxed.session_state),
        state::WORKING
    );
}

#[test]
fn open_name_attaches_without_touching_the_box() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let again = run(&w.cx, &req("demo")).unwrap();
    assert!(again.attached);
    assert_eq!(again.boxed.id, first.boxed.id);
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1"]);
    assert_eq!(w.surface.calls().len(), 2);
}

#[test]
fn attach_writes_exactly_one_attached_event() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let count = |w: &World| {
        w.cx.store()
            .unwrap()
            .events(BoxId::new(first.boxed.id), 100)
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
fn start_after_acquire_failure_gets_a_new_box() {
    let w = world(FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "boom".into(),
    }));
    run(&w.cx, &req("demo")).unwrap_err();
    let again = run(&w.cx, &req("demo")).unwrap();
    assert!(again.attached);
    assert_eq!(
        (again.boxed.lifecycle, again.boxed.session_state),
        state::WORKING
    );
    let row = w.cx.store().unwrap().box_row(BoxId::new(again.boxed.id));
    assert_eq!(row.unwrap().unwrap().boxed.unwrap().state, BoxState::Ready);
    assert_eq!(
        w.driver.calls(),
        ["acquire", "release box-1", "acquire", "exec box-1"]
    );
    assert_eq!(w.surface.calls().len(), 2);
}

#[test]
fn start_on_a_gone_box_acquires_again_and_keeps_the_box_working() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let id = BoxId::new(first.boxed.id);
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let again = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(
        (again.boxed.lifecycle, again.boxed.session_state),
        state::WORKING
    );
    let b = store.box_row(id).unwrap().unwrap().boxed.unwrap();
    assert_eq!(b.state, BoxState::Ready);
    assert_ne!(b.handle.unwrap().as_str(), "box-1");
    assert_eq!(
        w.driver.calls(),
        ["acquire", "exec box-1", "acquire", "exec box-2"]
    );
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
        "box_credentials: [missing]\n",
    )
    .unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(
        err.error
            .to_string()
            .contains("moor user secret set missing"),
        "{err:?}"
    );
    assert_eq!(
        err.value.as_ref().unwrap().pointer("/box/lifecycle"),
        Some(&serde_json::json!("rejected"))
    );
    assert!(w.surface.calls().is_empty());
    assert!(w.driver.calls().is_empty());
    let name = box_name("demo");
    assert!(w
        .cx
        .store()
        .unwrap()
        .open_box_by_name(&name)
        .unwrap()
        .is_none());
}

#[test]
fn start_refuses_while_another_op_holds_the_box_lock() {
    let w = world(FakeDriver::new());
    let _held = BoxLock::acquire(
        w.cx.clock(),
        w.cx.state_dir().unwrap(),
        BoxId::new(1),
        "close",
        Duration::ZERO,
    )
    .unwrap();
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), "box busy: close in progress");
    assert!(w.driver.calls().is_empty());
}

/// Holds the lock of Box 1 as `op`.
fn hold(w: &World, op: &str) -> BoxLock {
    BoxLock::acquire(
        w.cx.clock(),
        w.cx.state_dir().unwrap(),
        BoxId::new(1),
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
    assert_eq!(
        (again.boxed.lifecycle, again.boxed.session_state),
        state::WORKING
    );
    let row = w.cx.store().unwrap().box_row(BoxId::new(again.boxed.id));
    assert_eq!(row.unwrap().unwrap().boxed.unwrap().state, BoxState::Ready);
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1"]);
}

#[test]
fn start_on_a_lost_box_is_refused_while_another_op_holds_the_lock() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let id = BoxId::new(first.boxed.id);
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let _held = hold(&w, "close");
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), "box busy: close in progress");
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1"]);
}

#[test]
fn reprovision_skips_when_another_start_already_gave_the_box_back() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let id = BoxId::new(first.boxed.id);
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let stale = store.box_row(id).unwrap().unwrap();
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(
        w.driver.calls(),
        ["acquire", "exec box-1", "acquire", "exec box-2"]
    );
    let run =
        crate::ops::common::provision::reprovision(&w.cx, store, &stale, "start", false).unwrap();
    assert!(run.is_none());
    assert_eq!(
        w.driver.calls(),
        ["acquire", "exec box-1", "acquire", "exec box-2"]
    );
}

/// Starts `demo`, then leaves it as a provisioning that lost its Box handle.
fn stuck_provisioning(w: &World) {
    let first = run(&w.cx, &req("demo")).unwrap();
    let stuck = Decision::new(BoxId::new(first.boxed.id))
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
