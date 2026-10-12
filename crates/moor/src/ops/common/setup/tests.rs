//! Tests for the setup runner.

use std::path::Path;
use std::sync::Arc;

use moor_core::config::{Flags, TaskSettings, UserConfig};
use moor_core::domain::task::SetupOutcome;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan, ExecOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::view::SetupView;
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use super::{run_setup, SetupRun, SETUP_LINE_DEADLINE};
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
    codes.iter().map(|&code| Ok(ExecOutcome { code })).collect()
}

fn only<T>(mut v: Vec<T>) -> T {
    assert_eq!(v.len(), 1);
    v.remove(0)
}

#[test]
fn all_lines_passing_is_ok() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let got = run_setup(&r.cx, &r.handle, &r.workdir, &cfg(&["a", "b"], &[]));
    assert_eq!(got, SetupRun::Ok);
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn each_line_runs_once_in_order_under_sh() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        &cfg(&["one", "two", "three"], &[]),
    );
    let execs = r.driver.execs();
    let argvs: Vec<_> = execs.iter().map(|(_, s, _)| s.argv.clone()).collect();
    assert_eq!(
        argvs,
        vec![
            vec!["sh", "-c", "one"],
            vec!["sh", "-c", "two"],
            vec!["sh", "-c", "three"],
        ]
    );
    assert!(execs.iter().all(|(h, _, _)| *h == r.handle));
}

#[test]
fn runs_in_workdir_with_exactly_the_repo_env() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
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
fn every_line_gets_the_setup_deadline() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    run_setup(&r.cx, &r.handle, &r.workdir, &cfg(&["a", "b"], &[]));
    assert!(r
        .driver
        .execs()
        .iter()
        .all(|(_, _, dl)| *dl == Deadline(SETUP_LINE_DEADLINE)));
    assert_eq!(SETUP_LINE_DEADLINE.as_secs(), 1800);
}

#[test]
fn failing_line_stops_the_run_and_warns() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new().exec_results(exits(&[0, 3])));
    let got = run_setup(&r.cx, &r.handle, &r.workdir, &cfg(&["a", "bad", "c"], &[]));
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "bad".to_owned()
        }
    );
    assert_eq!(r.driver.execs().len(), 2);
    let w = only(r.cx.take_warnings());
    assert_eq!(w.code, "setup_failed");
    assert!(w.message.contains("bad"), "{}", w.message);
    assert!(w.message.contains("exit 3"), "{}", w.message);
}

#[test]
fn driver_timeout_fails_the_line() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(
        d.path(),
        FakeDriver::new().exec_results([Err(DriverError::Timeout(SETUP_LINE_DEADLINE))]),
    );
    let got = run_setup(&r.cx, &r.handle, &r.workdir, &cfg(&["slow", "next"], &[]));
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
    let got = run_setup(&r.cx, &r.handle, &r.workdir, &cfg(&[], &[("A", "1")]));
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
