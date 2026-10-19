use moor_core::domain::box_state::BoxLifecycle;
use std::fs;
use std::sync::Arc;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Tool;
use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::{BoxPath, CloseReq, ErrorKind};

use super::{admit, box_state, fx_with, proc, req, row, run, Fx};
use crate::cx::Cx;
use crate::ops::common::boxlock::{BoxLock, WAIT};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;
use crate::testing::proxy_host::FakeProxyHost;
use crate::testing::surface::FakeSurface;

pub(super) const YAML: &str =
    "teardown: ['echo bye', 'rm -rf cache']\nenv: {K: v}\nbox: {size: large}\n";

/// The Box user's HOME, which a Proxy-backed teardown stages its credentials under.
pub(super) fn with_home(driver: FakeDriver) -> FakeDriver {
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    driver.peek_text("box-1", &environ, "PATH=/bin\0HOME=/home/box\0")
}

fn fx_yaml(driver: FakeDriver, yaml: &str) -> Fx {
    let f = fx_with(with_home(driver), FakeSurface::new("/s"), 0);
    fs::write(f.dir.path().join(".moor.yaml"), yaml).unwrap();
    base_has(&f, yaml);
    f
}

/// Teardown runs; the unsynced unsaved-work probe is not counted.
fn teardown_execs(f: &Fx) -> Vec<ExecSpec> {
    f.driver
        .execs()
        .into_iter()
        .map(|(_, spec, _)| spec)
        .filter(|spec| spec.sync == SyncMode::Full)
        .collect()
}

pub(super) fn exec_calls(f: &Fx) -> usize {
    teardown_execs(f).len()
}

/// The single teardown exec the close made.
fn only_exec(f: &Fx) -> ExecSpec {
    let mut execs = teardown_execs(f);
    assert_eq!(execs.len(), 1);
    execs.remove(0)
}

#[test]
fn teardown_runs_as_one_synced_script_before_release() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    let spec = only_exec(&f);
    assert!(matches!(spec.argv.as_slice(), [.., sh, c, script]
        if sh == "sh" && c == "-c" && script.contains("echo bye") && script.contains("rm -rf cache")));
    assert_eq!(spec.sync, SyncMode::Full);
    assert_eq!(spec.host_cwd.as_deref(), Some(f.dir.path()));
    assert_eq!(spec.class.as_deref(), Some("large"));
    assert_eq!(spec.env.first(), Some(&("K".to_owned(), "v".to_owned())));
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn teardown_gets_the_proxy_variables_and_runs_in_the_workdir() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    run(&f.cx, &req(false)).unwrap();
    let spec = only_exec(&f);
    assert_eq!(spec.env.first(), Some(&("K".to_owned(), "v".to_owned())));
    assert_eq!(spec.cwd.unwrap().as_str(), "/work/box-1");
    assert!(matches!(spec.argv.as_slice(), [sh, c, script, ..]
        if sh == "sh" && c == "-c" && script.contains("export HTTPS_PROXY=")));
}

#[test]
fn teardown_runs_before_the_box_is_released() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    run(&f.cx, &req(false)).unwrap();
    let calls = f.driver.calls();
    let at = |p: &str| calls.iter().position(|c| c.starts_with(p)).unwrap();
    assert!(at("exec") < at("release"));
}

#[test]
fn failing_teardown_warns_and_still_releases() {
    let f = fx_yaml(
        FakeDriver::new().exec_results([
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 3 }),
        ]),
        YAML,
    );
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    let warnings = f.cx.take_warnings();
    assert!(warnings.iter().any(|w| w.code == "teardown_failed"));
}

#[test]
fn teardown_driver_error_warns_and_still_releases() {
    let f = fx_yaml(
        FakeDriver::new().exec_results([
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Err(DriverError::Failed {
                summary: "boom".into(),
            }),
        ]),
        YAML,
    );
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "teardown_failed"));
}

#[test]
fn interrupted_teardown_removes_the_staged_credential_dir() {
    let f = fx_yaml(
        FakeDriver::new().exec_results([
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Exited { code: 0 }),
            Ok(ExecOutcome::Interrupted),
        ]),
        YAML,
    );
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    let execs = f.driver.execs();
    assert_eq!(
        execs.len(),
        7,
        "probe, recheck, collection, artifacts, mkdir, teardown, rm"
    );
    let argv = &execs.last().unwrap().1.argv;
    assert!(
        matches!(argv.as_slice(), [rm, f, dir] if rm == "rm" && f == "-rf"
            && dir.starts_with("/home/box/.moor-cred-")),
        "{argv:?}"
    );
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "teardown_failed"));
}

#[test]
fn teardown_without_a_startable_proxy_warns_and_still_runs() {
    let mut f = fx_yaml(FakeDriver::new(), YAML);
    let host = FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("boom".into()));
    f.cx = f.cx.with_proxy_host(Arc::new(host));
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    let warnings = f.cx.take_warnings();
    let w = warnings
        .iter()
        .find(|w| w.code == "teardown_proxy")
        .unwrap();
    assert!(w.message.starts_with("teardown runs without the Proxy:"));
    assert!(script_of(&only_exec(&f)).contains("echo bye"));
}

#[test]
fn no_teardown_lines_runs_nothing() {
    let f = fx_yaml(FakeDriver::new(), "env: {K: v}\n");
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(exec_calls(&f), 0);
}

#[test]
fn gone_box_skips_teardown() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::DriverMissing))
        .unwrap();
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(exec_calls(&f), 0);
    assert_eq!(row(&f).lifecycle, BoxLifecycle::Closed);
}

#[test]
fn refused_close_runs_no_teardown() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 2);
    fs::write(f.dir.path().join(".moor.yaml"), YAML).unwrap();
    assert_eq!(
        run(&f.cx, &req(false)).unwrap_err().kind,
        ErrorKind::Refused
    );
    assert_eq!(exec_calls(&f), 0);
}

#[test]
fn close_waits_for_the_box_lock_then_refuses_when_busy() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    let _held =
        BoxLock::acquire(f.cx.clock(), f.cx.state_dir().unwrap(), f.id, "run", WAIT).unwrap();
    let from = f.cx.clock().instant();
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(e.error.to_string().contains("run"), "{}", e.error);
    assert!(f.cx.clock().instant() - from >= WAIT);
    assert_eq!(exec_calls(&f), 0);
    assert_ne!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn close_leaves_the_box_lock_free() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    run(&f.cx, &req(false)).unwrap();
    let again = BoxLock::acquire(
        f.cx.clock(),
        f.cx.state_dir().unwrap(),
        f.id,
        "start",
        std::time::Duration::ZERO,
    );
    assert!(again.is_ok());
}

/// A Ready Box in a worktree named `repo`, so a woken Box (which the fake driver
/// places at `/work/cbx_1/repo`) probes clean.
fn fx_woken_yaml(yaml: &str) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().join("repo");
    fs::create_dir(&worktree).unwrap();
    let path = BoxPath::under("/work/cbx_1/repo", ".moor/boxwork").unwrap();
    let driver =
        Arc::new(with_home(FakeDriver::new()).peek_text("box-1", &path, "ahead=0 dirty=0"));
    let surface = Arc::new(FakeSurface::new("/s"));
    let proc = proc(0);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        surface.clone(),
        proc.clone(),
    );
    let id = admit(&cx, &driver, &worktree);
    cx.store()
        .unwrap()
        .apply(Decision::new(id).set(Fields {
            box_workdir: Field::Set("/work/cbx_1/repo".to_owned()),
            ..Fields::default()
        }))
        .unwrap();
    fs::write(worktree.join(".moor.yaml"), yaml).unwrap();
    let f = Fx {
        dir,
        cx,
        driver,
        surface,
        proc,
        id,
    };
    base_has(&f, yaml);
    f
}

#[test]
fn asleep_box_is_woken_probed_and_released() {
    let f = fx_woken_yaml(YAML);
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::Sleep))
        .unwrap();
    assert_eq!(box_state(&f), BoxState::Asleep);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake")));
    // The woken Box is Ready again, so teardown runs on it like on any Ready Box.
    assert_eq!(exec_calls(&f), 1);
}

#[test]
fn invalid_repo_config_warns_and_still_closes_without_teardown() {
    let f = fx_yaml(FakeDriver::new(), "teardown: [\n");
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(exec_calls(&f), 0);
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "teardown_failed"));
}

const BASE_YAML: &str = "teardown: ['echo base']\n";

/// The Task has a base commit holding `base` as its `.moor.yaml`.
pub(super) fn base_has(f: &Fx, base: &str) {
    let fields = Fields {
        base: Field::Set("abc".into()),
        ..Fields::default()
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(fields))
        .unwrap();
    f.proc
        .before(Tool::Git, &["ls-tree"], Reply::Ok(".moor.yaml\n".into()));
    f.proc.before(Tool::Git, &["show"], Reply::Ok(base.into()));
}

fn script_of(spec: &ExecSpec) -> &str {
    spec.argv.last().map_or("", String::as_str)
}

#[test]
fn changed_moor_yaml_tears_down_with_the_base_copy_and_warns() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    base_has(&f, BASE_YAML);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    let spec = only_exec(&f);
    assert!(script_of(&spec).contains("echo base"));
    assert!(!script_of(&spec).contains("rm -rf cache"));
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "moor_yaml_untrusted"));
}

#[test]
fn trusted_changed_moor_yaml_tears_down_with_the_worktree_copy_and_warns() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    base_has(&f, BASE_YAML);
    let trusting = CloseReq {
        trust_moor_yaml: true,
        ..req(false)
    };
    let v = run(&f.cx, &trusting).unwrap();
    assert!(v.box_deleted);
    let spec = only_exec(&f);
    assert!(script_of(&spec).contains("rm -rf cache"));
    assert!(!script_of(&spec).contains("echo base"));
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "moor_yaml_trusted"));
}

#[test]
fn unchanged_moor_yaml_tears_down_without_a_moor_yaml_warning() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    base_has(&f, YAML);
    run(&f.cx, &req(false)).unwrap();
    let spec = only_exec(&f);
    assert!(script_of(&spec).contains("rm -rf cache"));
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .all(|w| !w.code.starts_with("moor_yaml_")));
}
