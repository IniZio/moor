//! Tests for `run`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moor_core::boxscript::{command_line, shell_argv};
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::proc::{Output, Tool};
use moor_core::{BoxId, BoxName, BoxPath, ErrorKind, ProviderId, RunReq};

use super::{locked, run};
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

mod env;
mod moor_yaml;
mod setup_parity;
mod transport;

pub(in crate::ops::run) const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

pub(crate) struct Fx {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) cx: Cx,
    pub(crate) driver: Arc<FakeDriver>,
    pub(in crate::ops::run) proc: Arc<ScriptedProc>,
    pub(in crate::ops::run) id: BoxId,
}

pub(crate) fn req(argv: &[&str]) -> RunReq {
    RunReq {
        box_name: BoxName::parse_allow_bare("t").unwrap(),
        record: None,
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        inherit_output: false,
        trust_moor_yaml: false,
    }
}

/// A context over `dir` with git answering a fixed HEAD.
fn context(dir: &std::path::Path, driver: &Arc<FakeDriver>) -> (Cx, Arc<ScriptedProc>) {
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok(format!("{SHA}\n")),
    );
    proc.on(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok(format!("{}/.git\n", dir.display())),
    );
    let cx = Cx::fake(
        dir,
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new(PathBuf::from("/s"))),
        proc.clone(),
    );
    (cx, proc)
}

/// A Box `t` with no machine yet.
fn bare_with(driver: FakeDriver) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let (cx, proc) = context(dir.path(), &driver);
    let id = cx
        .store()
        .unwrap()
        .apply(crate::testing::cx::new_box(
            BoxName::parse_allow_bare("t").unwrap(),
        ))
        .unwrap()
        .box_id;
    Fx {
        dir,
        cx,
        driver,
        proc,
        id,
    }
}

fn bare() -> Fx {
    bare_with(FakeDriver::new())
}

pub(in crate::ops::run) fn ready() -> Fx {
    ready_with(FakeDriver::new())
}

/// A Ready Box `t` with a machine from `driver`.
pub(crate) fn ready_with(driver: FakeDriver) -> Fx {
    let f = bare_with(driver);
    let store = f.cx.store().unwrap();
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set(f.dir.path().display().to_string()),
        base: Field::Set(SHA.to_owned()),
        ..Fields::default()
    };
    let d = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = f
        .driver
        .acquire(&AcquireReq {
            provider,
            box_id: f.id,
            name: BoxName::parse_allow_bare("t").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    f.driver
        .set_peek_text(got.handle.as_str(), &environ, "HOME=/home/box\0");
    let d = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            box_workdir: Field::Set(got.workdir.as_str().to_owned()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    f
}

/// Event kinds recorded for the Box, in order.
fn kinds(f: &Fx) -> Vec<String> {
    f.cx.store()
        .unwrap()
        .events(f.id, 1000)
        .unwrap()
        .iter()
        .map(|e| e.kind.clone())
        .collect()
}

pub(in crate::ops::run) fn ran_events(f: &Fx) -> usize {
    kinds(f).iter().filter(|k| *k == "ran").count()
}

#[test]
fn inherited_output_is_streamed_to_moor() {
    let f = ready_with(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 3 })]));
    let mut r = req(&["make", "test"]);
    r.inherit_output = true;
    let v = run(&f.cx, &r).unwrap();
    assert_eq!(v.exit, Some(3));
    assert!(v.ran && !v.interrupted);
    let (_, spec, _) = f.driver.execs().remove(0);
    assert_eq!(spec.output, Some(Output::Inherit));
    assert_eq!(command(&spec.argv), shell_argv("'make' 'test'"));
}

#[test]
fn plain_run_launches_shell_text_and_stores_the_users_argv() {
    let (f, multi) = (ready(), req(&["echo", "a b"]));
    run(&f.cx, &req(&["echo one; echo TWO"])).unwrap();
    run(&f.cx, &multi).unwrap();
    let launched = |i| command(&f.driver.execs().remove(i).1.argv).to_vec();
    assert_eq!(launched(0), shell_argv("echo one; echo TWO"));
    assert_eq!(launched(1), shell_argv(&command_line(&multi.argv)));
    let stored = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap().argv;
    assert_eq!(stored, multi.argv);
}

#[test]
fn json_output_goes_to_a_run_log_under_the_state_dir() {
    let f = ready();
    let v = run(&f.cx, &req(&["true"])).unwrap();
    let id = v.run.unwrap();
    let want = f.dir.path().join("runs").join(format!("{}.log", id.get()));
    assert_eq!(v.log, want.display().to_string());
    assert!(want.parent().unwrap().is_dir());
    let (handle, spec, _) = f.driver.execs().remove(0);
    assert_eq!(handle.as_str(), "box-1");
    assert_eq!(spec.output, Some(Output::Log(want)));
    assert_eq!(v.exit, Some(0));
    assert_eq!(v.record, None);
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn a_plain_run_syncs_nothing_stores_no_sha_and_takes_its_class_and_directories_from_the_box() {
    let f = ready();
    let v = run(&f.cx, &req(&["true"])).unwrap();
    assert_eq!(v.sha, None);
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.sha, None);
    let (_, spec, _) = f.driver.execs().remove(0);
    assert_eq!(spec.host_cwd.as_deref(), Some(f.dir.path()));
    assert_eq!(spec.cwd.unwrap().as_str(), "/work/cbx_1/repo");
    assert_eq!(spec.class.as_deref(), Some("small"));
}

/// The user's command inside the exec prelude wrapper; the argv itself when unwrapped.
pub(in crate::ops::run) fn command(argv: &[String]) -> &[String] {
    match argv {
        [_, _, script, sh, rest @ ..] if sh == "sh" && script.ends_with("\"$@\"") => rest,
        _ => argv,
    }
}

fn refused(f: &Fx, r: &RunReq, kind: ErrorKind) -> String {
    let e = run(&f.cx, r).unwrap_err();
    assert_eq!(e.kind, kind, "{e:?}");
    assert!(f.driver.calls().iter().all(|c| !c.starts_with("exec")));
    e.error.to_string()
}

#[test]
fn unknown_box_is_refused() {
    let f = bare();
    let mut r = req(&["true"]);
    r.box_name = BoxName::parse_allow_bare("nope").unwrap();
    let before = f.driver.calls().len();
    assert_eq!(refused(&f, &r, ErrorKind::Failed), "no Box nope");
    assert_eq!(f.driver.calls().len(), before);
}

#[test]
fn stored_workdir_with_dot_dot_is_the_command_cwd() {
    let f = ready_with(FakeDriver::new().work_root("/p/bin/../box/work"));
    run(&f.cx, &req(&["true"])).unwrap();
    let (_, spec, _) = f.driver.execs().remove(0);
    assert_eq!(spec.cwd.unwrap().as_str(), "/p/bin/../box/work/cbx_1/repo");
}

#[test]
fn empty_argv_is_usage() {
    let f = ready();
    let before = f.driver.calls().len();
    refused(&f, &req(&[]), ErrorKind::Usage);
    assert_eq!(f.driver.calls().len(), before);
}

#[test]
fn box_without_a_box_is_refused() {
    let f = bare();
    assert_eq!(
        refused(&f, &req(&["true"]), ErrorKind::Failed),
        "t has no box"
    );
    assert!(f.driver.calls().is_empty());
}

#[test]
fn box_waiting_for_a_box_is_refused() {
    let f = bare();
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).lifecycle(BoxLifecycleInput::AtCapacity { max_boxes: 1 }))
        .unwrap();
    let msg = refused(&f, &req(&["true"]), ErrorKind::Refused);
    assert_eq!(msg, "t is waiting for a free box");
    assert!(f.driver.calls().is_empty());
}

#[test]
fn gone_box_is_refused_and_recorded() {
    let f = ready();
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let before = f.driver.calls().len();
    let msg = refused(&f, &req(&["true"]), ErrorKind::Failed);
    assert!(msg.contains("gone"), "{msg}");
    assert_eq!(f.driver.calls().len(), before);
    assert_eq!(ran_events(&f), 1);
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
}

/// Put the Box and machine to sleep.
fn sleep(f: &Fx) {
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).session(SessionInput::AgentQuiet))
        .unwrap();
    store
        .apply(
            Decision::new(f.id)
                .lifecycle(BoxLifecycleInput::Sleep { auto: false })
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
}

fn wake_calls(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("wake"))
        .count()
}

fn box_state(f: &Fx) -> BoxState {
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    row.boxed.unwrap().state
}

#[test]
fn asleep_box_is_woken_before_the_command_runs() {
    let f = ready();
    sleep(&f);
    let v = run(&f.cx, &req(&["true"])).unwrap();
    assert!(v.ran);
    assert_eq!(wake_calls(&f), 1);
    assert_eq!(f.driver.execs().len(), 1);
    assert_eq!(box_state(&f), BoxState::Ready);
    assert!(kinds(&f).contains(&"woke".to_owned()));
}

#[test]
fn failed_wake_refuses_the_run_and_leaves_the_box_asleep() {
    let f = ready_with(FakeDriver::new().fail_wake(DriverError::NotFound));
    sleep(&f);
    run(&f.cx, &req(&["true"])).unwrap_err();
    assert_eq!(wake_calls(&f), 1);
    assert!(f.driver.execs().is_empty());
    assert_eq!(box_state(&f), BoxState::Asleep);
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
    assert!(!kinds(&f).contains(&"woke".to_owned()));
}

#[test]
fn gone_asleep_box_is_refused_without_a_wake() {
    let f = ready();
    sleep(&f);
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let msg = refused(&f, &req(&["true"]), ErrorKind::Failed);
    assert!(msg.contains("gone"), "{msg}");
    assert_eq!(wake_calls(&f), 0);
    assert_eq!(ran_events(&f), 1);
    assert!(f.driver.execs().is_empty());
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
}

#[test]
fn ready_box_is_not_woken() {
    let f = ready();
    run(&f.cx, &req(&["true"])).unwrap();
    assert_eq!(wake_calls(&f), 0);
}

#[test]
fn dead_provisioning_is_refused_with_a_close_hint() {
    let f = bare();
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).intent(IntentChange::Set(Intent::Provisioning { pid: 4242 })))
        .unwrap();
    f.proc.dead(4242);
    f.cx.clock().sleep(Duration::from_secs(121));
    let e = run(&f.cx, &req(&["true"])).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert_eq!(e.error.to_string(), "provisioning dead; run moor close t");
    assert!(e.hints.iter().any(|h| h == "moor close t"), "{e:?}");
    assert!(f.driver.calls().is_empty());
}

#[test]
fn provisioning_that_died_while_waiting_for_the_lock_is_refused() {
    let f = bare();
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).intent(IntentChange::Set(Intent::Provisioning { pid: 4242 })))
        .unwrap();
    f.proc.dead(4242);
    f.cx.clock().sleep(Duration::from_secs(121));
    let row = store.box_row(f.id).unwrap().unwrap();
    let e = locked(&f.cx, store, &req(&["true"]), &row, None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert_eq!(e.error.to_string(), "provisioning dead; run moor close t");
    assert!(f.driver.calls().iter().all(|c| !c.starts_with("exec")));
}

#[test]
fn busy_box_is_refused() {
    let f = ready();
    let _held =
        BoxLock::acquire(f.cx.clock(), f.dir.path(), f.id, "start", Duration::ZERO).unwrap();
    let msg = refused(&f, &req(&["true"]), ErrorKind::Refused);
    assert!(msg.starts_with("box busy"), "{msg}");
}

#[test]
fn run_is_recorded_in_flight_while_the_command_runs() {
    let f = ready();
    let cx_dir = f.dir.path().to_path_buf();
    let seen = Arc::new(std::sync::Mutex::new(None));
    let slot = seen.clone();
    let id = f.id;
    f.driver.on_exec(move || {
        let probe = Cx::fake(
            &cx_dir,
            &cx_dir,
            Arc::new(FakeDriver::new()),
            Arc::new(FakeSurface::new(PathBuf::from("/s"))),
            Arc::new(ScriptedProc::new()),
        );
        let row = probe.store().unwrap().last_run(id).unwrap();
        *slot.lock().unwrap() = row.map(|r| (r.argv, r.finished_at.is_none()));
    });
    run(&f.cx, &req(&["sleep", "1"])).unwrap();
    let got = seen.lock().unwrap().clone().unwrap();
    assert_eq!(got, (vec!["sleep".to_owned(), "1".to_owned()], true));
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.finished_at.is_some());
    assert_eq!(row.exit, Some(0));
}

#[test]
fn interrupted_command_is_recorded_without_an_exit() {
    let f = ready_with(FakeDriver::new().exec_results([Ok(ExecOutcome::Interrupted)]));
    let v = run(&f.cx, &req(&["sleep", "9"])).unwrap();
    assert!(v.interrupted && v.ran);
    assert_eq!(v.exit, None);
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.interrupted);
    assert_eq!(row.exit, None);
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn driver_failure_finishes_the_run_and_returns_the_error() {
    let f = ready_with(FakeDriver::new().exec_results([Err(DriverError::Failed {
        summary: "sync failed".into(),
    })]));
    let e = run(&f.cx, &req(&["true"])).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert_eq!(e.error.to_string(), "sync failed");
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.finished_at.is_some());
    assert_eq!((row.exit, row.interrupted), (None, false));
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn driver_failure_is_kept_on_the_ran_event() {
    let f = ready_with(FakeDriver::new().exec_results([Err(DriverError::Failed {
        summary: "the Box lease was busy; try again".into(),
    })]));
    run(&f.cx, &req(&["true"])).unwrap_err();
    let events = f.cx.store().unwrap().events(f.id, 1000).unwrap();
    let ran = events.iter().find(|e| e.kind == "ran").unwrap();
    assert_eq!(
        ran.payload,
        serde_json::json!({
            "run": f.cx.store().unwrap().last_run(f.id).unwrap().unwrap().id.get(),
            "exit": null,
            "interrupted": false,
            "error": "the Box lease was busy; try again",
        })
    );
}

#[test]
fn unusable_repo_config_leaves_no_run_row() {
    let f = ready();
    let text = "box_credentials: [\n";
    std::fs::write(f.dir.path().join(".moor.yaml"), text).unwrap();
    f.proc
        .before(Tool::Git, &["ls-tree"], Reply::Ok(".moor.yaml\n".into()));
    f.proc.before(Tool::Git, &["show"], Reply::Ok(text.into()));
    let before = f.driver.calls().len();
    refused(&f, &req(&["true"]), ErrorKind::Failed);
    assert_eq!(f.driver.calls().len(), before);
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
    assert_eq!(ran_events(&f), 0);
}

#[test]
fn unreadable_head_records_and_reports_no_sha() {
    let f = ready();
    let proc = Arc::new(ScriptedProc::new());
    proc.on(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok(format!("{}/.git\n", f.dir.path().display())),
    );
    let cx = Cx::fake(
        f.dir.path(),
        f.dir.path(),
        f.driver.clone(),
        Arc::new(FakeSurface::new(PathBuf::from("/s"))),
        proc,
    );
    let v = run(&cx, &req(&["true"])).unwrap();
    assert_eq!(v.sha, None);
    let row = cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.sha, None);
}

/// The Box's `idle_since` column, read over a second connection.
fn idle_since(f: &Fx) -> Option<i64> {
    let db = rusqlite::Connection::open(f.dir.path().join("moor.db")).unwrap();
    db.query_row(
        "SELECT idle_since FROM boxes WHERE id = ?",
        [f.id.get()],
        |r| r.get(0),
    )
    .unwrap()
}

fn state(f: &Fx) -> (BoxLifecycle, SessionState) {
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    (row.lifecycle, row.session)
}

fn go_idle(f: &Fx) {
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).session(SessionInput::AgentQuiet))
        .unwrap();
}

#[test]
fn run_on_an_idle_box_leaves_it_working() {
    let f = ready();
    go_idle(&f);
    assert!(idle_since(&f).is_some());
    run(&f.cx, &req(&["true"])).unwrap();
    assert_eq!(state(&f), (BoxLifecycle::Ready, SessionState::Working));
    assert_eq!(idle_since(&f), None);
    assert_eq!(kinds(&f).iter().filter(|k| *k == "busy").count(), 1);
}

#[test]
fn run_on_a_working_box_applies_nothing() {
    let f = ready();
    let busy = |f: &Fx| kinds(f).iter().filter(|k| *k == "busy").count();
    run(&f.cx, &req(&["true"])).unwrap();
    assert_eq!(state(&f), (BoxLifecycle::Ready, SessionState::Working));
    assert_eq!(busy(&f), 0);
}

#[test]
fn run_on_an_asleep_box_wakes_it_without_run_started() {
    let f = ready();
    sleep(&f);
    run(&f.cx, &req(&["true"])).unwrap();
    assert_eq!(state(&f), (BoxLifecycle::Ready, SessionState::Working));
}

#[test]
fn run_started_losing_a_race_is_ignored() {
    let f = ready();
    go_idle(&f);
    let store = f.cx.store().unwrap();
    let stale = store.box_row(f.id).unwrap().unwrap();
    store
        .apply(Decision::new(f.id).session(SessionInput::AgentBusy))
        .unwrap();
    let v = locked(&f.cx, store, &req(&["true"]), &stale, None).unwrap();
    assert!(v.ran);
    assert_eq!(state(&f), (BoxLifecycle::Ready, SessionState::Working));
}
