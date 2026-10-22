//! Tests for `run`.

use std::sync::Arc;
use std::time::Duration;

use moor_core::boxscript::{command_line, shell_argv};
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::{BoxId, BoxName, BoxPath, ErrorKind, ProviderId, RunId, RunReq};

use super::{locked, run};
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::proxy_host::FakeProxyHost;
use crate::testing::surface::FakeSurface;
use moor_testkit::FakeEgress;

mod detached;
mod env;
mod in_flight;
mod setup_parity;
mod transport;

pub(in crate::ops::run) const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

pub(crate) struct Fx {
    pub(crate) dir: tempfile::TempDir,
    /// The Box's `HOME` (`home`) and workdir (`work`), where its runs play out on this host.
    pub(crate) box_dir: tempfile::TempDir,
    /// The egress proxy the Box's runs find in `$HOME/bin`.
    pub(crate) egress: FakeEgress,
    pub(crate) cx: Cx,
    pub(crate) driver: Arc<FakeDriver>,
    pub(in crate::ops::run) proc: Arc<ScriptedProc>,
    pub(in crate::ops::run) id: BoxId,
}

/// A command that kills its run's group leader in the Box, so the run leaves no exit code. The
/// fixture's first run is run 1.
pub(in crate::ops::run) const LEADER_KILLED: &str =
    r#"echo before; kill -s KILL "$(cat "$HOME/.moor/runs/1/pid")"; sleep 5"#;

/// Makes the fixture Box's workdir a git checkout with one commit of `tracked`; returns its sha.
pub(in crate::ops::run) fn git_checkout(f: &Fx) -> String {
    let work = f.box_dir.path().join("work");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&work)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-q"]);
    std::fs::write(work.join("tracked"), "one\n").unwrap();
    git(&["add", "tracked"]);
    git(&["commit", "-qm", "one"]);
    git(&["rev-parse", "HEAD"]).trim().to_owned()
}

pub(crate) fn req(argv: &[&str]) -> RunReq {
    RunReq {
        box_name: BoxName::parse_allow_bare("t").unwrap(),
        record: None,
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        inherit_output: false,
    }
}

/// A context over `dir` whose host processes answer no tool and whose Proxy names `egress_port`.
fn context(
    dir: &std::path::Path,
    driver: &Arc<FakeDriver>,
    egress_port: u16,
) -> (Cx, Arc<ScriptedProc>) {
    let proc = Arc::new(ScriptedProc::new());
    let cx = Cx::fake(
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new()),
        proc.clone(),
    )
    .with_proxy_host(Arc::new(FakeProxyHost::new().egress_port(egress_port)));
    (cx, proc)
}

/// A Box `t` of repo `o/n` with no machine yet; `yaml` is its stored `.moor.yaml`. Its runs play
/// out with the local sh under `box_dir`, with a fake egress proxy.
fn bare_with(driver: FakeDriver, yaml: Option<&str>) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let box_dir = tempfile::tempdir().unwrap();
    let (home, work) = (box_dir.path().join("home"), box_dir.path().join("work"));
    std::fs::create_dir_all(&work).unwrap();
    let bin = home.join("bin");
    moor_testkit::write_host_guards(&bin).unwrap();
    let egress = FakeEgress::start(&home, &bin).unwrap();
    let driver = Arc::new(driver.local_runs(&home, &work));
    let (cx, proc) = context(dir.path(), &driver, egress.port());
    let id = cx
        .store()
        .unwrap()
        .apply(crate::testing::cx::new_box_of(
            BoxName::parse_allow_bare("t").unwrap(),
            crate::testing::cx::box_repo(yaml),
        ))
        .unwrap()
        .box_id;
    Fx {
        dir,
        box_dir,
        egress,
        cx,
        driver,
        proc,
        id,
    }
}

impl Fx {
    /// The Box's run dir of run `id`.
    pub(crate) fn box_run_dir(&self, id: i64) -> std::path::PathBuf {
        self.box_dir.path().join(format!("home/.moor/runs/{id}"))
    }

    /// The host log of run `id`.
    pub(crate) fn host_log(&self, id: RunId) -> Vec<u8> {
        std::fs::read(self.dir.path().join(format!("runs/{}.log", id.get()))).unwrap()
    }

    /// The Box log of run `id`.
    pub(crate) fn box_log(&self, id: RunId) -> Vec<u8> {
        std::fs::read(self.box_run_dir(id.get()).join("log")).unwrap()
    }
}

fn bare() -> Fx {
    bare_with(FakeDriver::new(), None)
}

pub(in crate::ops::run) fn ready() -> Fx {
    ready_with(FakeDriver::new())
}

/// A Ready Box `t` with a machine from `driver`.
pub(crate) fn ready_with(driver: FakeDriver) -> Fx {
    ready_repo(driver, None)
}

/// A Ready Box `t` whose stored `.moor.yaml` is `yaml`.
pub(in crate::ops::run) fn ready_yaml(yaml: &str) -> Fx {
    ready_repo(FakeDriver::new(), Some(yaml))
}

/// A Ready Box `t` with a machine from `driver`; `yaml` is its stored `.moor.yaml`.
pub(in crate::ops::run) fn ready_repo(driver: FakeDriver, yaml: Option<&str>) -> Fx {
    let f = bare_with(driver, yaml);
    let store = f.cx.store().unwrap();
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
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
    let f = ready();
    let mut r = req(&["sh", "-c", "exit 3"]);
    r.inherit_output = true;
    let v = run(&f.cx, &r).unwrap();
    assert_eq!(v.exit, Some(3));
    assert!(v.ran && !v.interrupted);
    assert_eq!(v.log, "");
    assert!(!f.dir.path().join("runs/1.log").exists(), "no host log");
    let (_, spec) = f.driver.launches().remove(0);
    assert_eq!(command(&spec.argv), shell_argv("'sh' '-c' 'exit 3'"));
}

#[test]
fn plain_run_launches_shell_text_and_stores_the_users_argv() {
    let (f, multi) = (ready(), req(&["echo", "a b"]));
    run(&f.cx, &req(&["echo one; echo TWO"])).unwrap();
    run(&f.cx, &multi).unwrap();
    let launched = |i| command(&f.driver.launches().remove(i).1.argv).to_vec();
    assert_eq!(launched(0), shell_argv("echo one; echo TWO"));
    assert_eq!(launched(1), shell_argv(&command_line(&multi.argv)));
    let stored = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap().argv;
    assert_eq!(stored, multi.argv);
}

#[test]
fn json_output_goes_to_a_run_log_under_the_state_dir() {
    let f = ready();
    let v = run(&f.cx, &req(&["echo hi"])).unwrap();
    let id = v.run.unwrap();
    let want = f.dir.path().join("runs").join(format!("{}.log", id.get()));
    assert_eq!(v.log, want.display().to_string());
    assert_eq!(std::fs::read(&want).unwrap(), b"hi\n");
    let (handle, _) = f.driver.launches().remove(0);
    assert_eq!(handle.as_str(), "box-1");
    assert_eq!(v.exit, Some(0));
    assert_eq!(v.record, None);
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn a_plain_run_syncs_nothing_stores_no_sha_and_takes_its_directories_from_the_box() {
    let f = ready();
    let v = run(&f.cx, &req(&["true"])).unwrap();
    assert_eq!(v.sha, None);
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.sha, None);
    let (_, spec) = f.driver.launches().remove(0);
    assert_eq!(spec.cwd.unwrap().as_str(), "/work/cbx_1/repo");
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
    let (_, spec) = f.driver.launches().remove(0);
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
    assert_eq!(f.driver.launches().len(), 1);
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
fn driver_failure_finishes_the_run_and_returns_the_error() {
    let f = ready_with(FakeDriver::new().fail_launch(DriverError::Failed {
        summary: "sync failed".into(),
    }));
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
    let f = ready_with(FakeDriver::new().fail_launch(DriverError::Failed {
        summary: "the Box lease was busy; try again".into(),
    }));
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
    let f = ready_yaml("box_credentials: [\n");
    let before = f.driver.calls().len();
    refused(&f, &req(&["true"]), ErrorKind::Failed);
    assert_eq!(f.driver.calls().len(), before);
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
    assert_eq!(ran_events(&f), 0);
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
