//! Tests for the setup runner.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::boxscript::setup::setup_script;
use moor_core::config::{Flags, TaskSettings, UserConfig};
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::SetupOutcome;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan, ExecOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::view::SetupView;
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use super::{run_setup, setup_deadline, SetupRun, SETUP_LINE_DEADLINE};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Rig {
    cx: Cx,
    driver: Arc<FakeDriver>,
    handle: BoxHandle,
    workdir: BoxPath,
}

fn rig(dir: &Path, driver: FakeDriver) -> Rig {
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir,
        dir,
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new(dir)),
        Arc::new(ScriptedProc::new()),
    );
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: TaskId::new(1),
            name: TaskName::parse("demo").unwrap(),
            egress: EgressPlan::Open,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    Rig {
        cx,
        driver,
        handle: got.handle,
        workdir: got.workdir,
    }
}

fn cfg(setup: &[&str], env: &[(&str, &str)]) -> TaskSettings {
    let mut c = TaskSettings::resolve(&UserConfig::default(), None, &Flags::default());
    c.setup = setup.iter().map(|s| (*s).to_owned()).collect();
    c.env = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    c
}

fn exits(codes: &[i32]) -> Vec<Result<ExecOutcome, DriverError>> {
    codes
        .iter()
        .map(|&code| Ok(ExecOutcome::Exited { code }))
        .collect()
}

fn only<T>(mut v: Vec<T>) -> T {
    assert_eq!(v.len(), 1);
    v.remove(0)
}

#[test]
fn all_lines_passing_is_ok() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a", "b"], &[]),
    );
    assert_eq!(got, SetupRun::Ok);
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn lines_run_as_one_generated_script_under_sh() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let c = cfg(&["one", "two", "three"], &[]);
    run_setup(&r.cx, &r.handle, &r.workdir, d.path(), &c);
    let (h, spec, _) = only(r.driver.execs());
    assert_eq!(h, r.handle);
    assert_eq!(
        spec.argv,
        vec!["sh".to_owned(), "-c".to_owned(), setup_script(&c.setup)]
    );
}

#[test]
fn runs_in_workdir_with_exactly_the_repo_env() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[("B", "2"), ("A", "1")]),
    );
    let (_, spec, _) = only(r.driver.execs());
    assert_eq!(spec.cwd.as_ref(), Some(&r.workdir));
    assert_eq!(
        spec.env,
        vec![
            ("A".to_owned(), "1".to_owned()),
            ("B".to_owned(), "2".to_owned())
        ]
    );
}

#[test]
fn the_script_gets_one_line_allowance_per_line() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a", "b", "c"], &[]),
    );
    let (_, _, dl) = only(r.driver.execs());
    assert_eq!(dl, Deadline(SETUP_LINE_DEADLINE * 3));
    assert_eq!(SETUP_LINE_DEADLINE.as_secs(), 1800);
    assert_eq!(setup_deadline(0), Duration::ZERO);
}

/// Makes every exec leave `log` as the script's log in the worktree.
fn leave_log(r: &Rig, dir: &Path, log: &'static str) {
    let dir = dir.to_path_buf();
    r.driver.on_exec(move || {
        std::fs::create_dir_all(dir.join(".moor")).unwrap();
        std::fs::write(dir.join(".moor/setup.log"), log).unwrap();
    });
}

#[test]
fn failing_line_comes_from_the_log_and_warns() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new().exec_results(exits(&[3])));
    leave_log(
        &r,
        d.path(),
        "moor setup: $ a\nmoor setup: $ bad\nmoor setup: step 2 exited 3\n",
    );
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a", "bad", "c"], &[]),
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "bad".to_owned()
        }
    );
    assert_eq!(r.driver.execs().len(), 1);
    let w = only(r.cx.take_warnings());
    assert_eq!(w.code, "setup_failed");
    assert!(w.message.contains("bad"), "{}", w.message);
    assert!(w.message.contains("exit 3"), "{}", w.message);
}

#[test]
fn without_a_failure_marker_the_last_announced_line_fails() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new().exec_results(exits(&[1])));
    leave_log(&r, d.path(), "moor setup: $ a\nmoor setup: $ b\n");
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a", "b", "c"], &[]),
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "b".to_owned()
        }
    );
}

#[test]
fn without_a_log_the_first_line_fails() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new().exec_results(exits(&[1])));
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a", "b"], &[]),
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "a".to_owned()
        }
    );
}

#[test]
fn driver_timeout_fails_the_line() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(
        d.path(),
        FakeDriver::new().exec_results([Err(DriverError::Timeout(SETUP_LINE_DEADLINE))]),
    );
    leave_log(&r, d.path(), "moor setup: $ slow\n");
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["slow", "next"], &[]),
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "slow".to_owned()
        }
    );
    assert_eq!(r.driver.execs().len(), 1);
    let w = only(r.cx.take_warnings());
    assert_eq!(w.code, "setup_failed");
    assert!(w.message.contains("slow"), "{}", w.message);
    let m = w.message.to_lowercase();
    assert!(m.contains("timeout") || m.contains("timed out"), "{m}");
}

#[test]
fn empty_setup_is_skipped_without_exec() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&[], &[("A", "1")]),
    );
    assert_eq!(got, SetupRun::Skipped);
    assert!(r.driver.execs().is_empty());
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn outcome_maps_skipped_and_ok_to_ok() {
    assert_eq!(SetupRun::Skipped.outcome(), SetupOutcome::Ok);
    assert_eq!(SetupRun::Ok.outcome(), SetupOutcome::Ok);
    assert_eq!(
        SetupRun::Failed {
            line: "x".to_owned()
        }
        .outcome(),
        SetupOutcome::Failed {
            line: "x".to_owned()
        }
    );
}

#[test]
fn view_maps_each_variant() {
    assert_eq!(SetupRun::Skipped.view(), SetupView::default());
    assert_eq!(
        SetupRun::Ok.view(),
        SetupView {
            ran: true,
            ok: true,
            line: None
        }
    );
    assert_eq!(
        SetupRun::Failed {
            line: "x".to_owned()
        }
        .view(),
        SetupView {
            ran: true,
            ok: false,
            line: Some("x".to_owned())
        }
    );
}

#[test]
fn failed_is_true_only_for_failed() {
    assert!(!SetupRun::Skipped.failed());
    assert!(!SetupRun::Ok.failed());
    assert!(SetupRun::Failed {
        line: "x".to_owned()
    }
    .failed());
}

#[test]
fn exec_is_synced_from_the_worktree_at_the_resolved_size() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let c = cfg(&["a"], &[]);
    run_setup(&r.cx, &r.handle, &r.workdir, d.path(), &c);
    let (_, spec, _) = only(r.driver.execs());
    assert_eq!(spec.sync, SyncMode::Full);
    assert!(spec.output.is_none());
    assert_eq!(spec.host_cwd.as_deref(), Some(d.path()));
    assert_eq!(spec.class.as_deref(), Some(c.size.value.as_str()));
}

#[test]
fn interrupted_line_fails_and_stops_the_run() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(ExecOutcome::Interrupted)]);
    let r = rig(d.path(), driver);
    leave_log(&r, d.path(), "moor setup: $ a\n");
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a", "b"], &[]),
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "a".to_owned()
        }
    );
    assert_eq!(r.driver.execs().len(), 1);
    let warnings = r.cx.take_warnings();
    assert_eq!(warnings.len(), 1);
    let w = warnings.first().unwrap();
    assert_eq!(w.code, "setup_failed");
    assert!(w.message.contains("interrupted"));
}
