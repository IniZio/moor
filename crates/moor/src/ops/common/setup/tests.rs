//! Tests for the setup runner.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::boxscript::setup::setup_script;
use moor_core::config::{BoxSettings, Flags, UserConfig};
use moor_core::domain::box_state::SetupOutcome;
use moor_core::exec_env::SETUP_LINE_DEADLINE;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::view::SetupView;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use super::{run_setup as run_setup_with, SetupRun};
use crate::cx::Cx;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

mod output;
mod proxy;
mod warning;

struct Rig {
    cx: Cx,
    driver: Arc<FakeDriver>,
    handle: BoxHandle,
    workdir: BoxPath,
}

fn rig(dir: &Path, driver: FakeDriver) -> Rig {
    rig_on(dir, driver, Arc::new(FakeClock::new(Ts::from_millis(1000))))
}

fn rig_on(dir: &Path, driver: FakeDriver, clock: Arc<FakeClock>) -> Rig {
    let driver = Arc::new(driver);
    let cx = Cx::fake_with_clock(
        dir,
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        UserConfig::default(),
        clock,
    );
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            box_id: BoxId::new(1),
            name: BoxName::parse_allow_bare("demo").unwrap(),
            class: None,
            nonce: "n".to_owned(),
        })
        .unwrap();
    Rig {
        cx,
        driver,
        handle: got.handle,
        workdir: got.workdir,
    }
}

fn cfg(setup: &[&str], env: &[(&str, &str)]) -> BoxSettings {
    let mut c = BoxSettings::resolve(&UserConfig::default(), None, &Flags::default());
    c.setup = setup.iter().map(|s| (*s).to_owned()).collect();
    c.env = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    c
}

/// Runs setup in the rig's Box without a Proxy, keeping its log under the state dir `state`.
fn run_setup(
    cx: &Cx,
    handle: &BoxHandle,
    workdir: &BoxPath,
    state: &Path,
    cfg: &BoxSettings,
) -> SetupRun {
    let logs = super::log_dir(state, BoxId::new(1));
    run_setup_with(cx, handle, "p", workdir, &logs, cfg, None)
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
    assert_eq!(
        got,
        SetupRun::Ok {
            ms: 0,
            tail: String::new(),
        }
    );
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
    // The wrapper puts the Box's image PATH on the exec first; the script is the last argument.
    assert_eq!(spec.argv.get(..2).unwrap(), ["sh", "-c"]);
    assert!(spec.argv.get(2).unwrap().contains("/etc/environment"));
    assert_eq!(
        spec.argv.get(3..).unwrap(),
        ["sh", "sh", "-c", setup_script(&c.setup).as_str()]
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
}

#[test]
fn failing_line_comes_from_the_log_and_warns() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[3]))
        .copy_text("moor setup: $ a\nmoor setup: $ bad\n");
    let r = rig(d.path(), driver);
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
            line: "bad".to_owned(),
            exit: Some(3),
            ms: 0,
            tail: "moor setup: $ a\nmoor setup: $ bad".to_owned(),
        }
    );
    assert_eq!(r.driver.execs().len(), 1);
    let (h, from, to) = only(r.driver.copies());
    assert_eq!(h, r.handle);
    assert_eq!(
        from.as_str(),
        format!("{}/.moor/setup.log", r.workdir.as_str())
    );
    assert_eq!(to, d.path().join("logs/1/setup.log"));
    let w = only(r.cx.take_warnings());
    assert_eq!(w.code, "setup_failed");
    assert!(w.message.contains("bad"), "{}", w.message);
    assert!(w.message.contains("exit 3"), "{}", w.message);
}

#[test]
fn the_last_announced_line_fails() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .copy_text("moor setup: $ a\nmoor setup: $ b\n");
    let r = rig(d.path(), driver);
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
            line: "b".to_owned(),
            exit: Some(1),
            ms: 0,
            tail: "moor setup: $ a\nmoor setup: $ b".to_owned(),
        }
    );
}

#[test]
fn a_failed_copy_falls_back_to_the_first_line() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .fail_copy_out(DriverError::NotFound);
    let r = rig(d.path(), driver);
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
            line: "a".to_owned(),
            exit: Some(1),
            ms: 0,
            tail: String::new(),
        }
    );
}

#[test]
fn driver_timeout_fails_the_line() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results([Err(DriverError::Timeout(SETUP_LINE_DEADLINE))])
        .copy_text("moor setup: $ slow\n");
    let r = rig(d.path(), driver);
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
            line: "slow".to_owned(),
            exit: None,
            ms: 0,
            tail: "moor setup: $ slow".to_owned(),
        }
    );
    assert_eq!(r.driver.execs().len(), 1);
    let w = only(r.cx.take_warnings());
    assert_eq!(w.code, "setup_failed");
    assert!(w.message.contains("slow"), "{}", w.message);
    let m = w.message.to_lowercase();
    assert!(m.contains("timeout") || m.contains("timed out"), "{m}");
    assert_eq!(r.driver.copies().len(), 1);
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
    assert_eq!(
        SetupRun::Ok {
            ms: 0,
            tail: String::new(),
        }
        .outcome(),
        SetupOutcome::Ok
    );
    assert_eq!(
        SetupRun::Failed {
            line: "x".to_owned(),
            exit: None,
            ms: 0,
            tail: String::new(),
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
        SetupRun::Ok {
            ms: 0,
            tail: String::new(),
        }
        .view(),
        SetupView {
            ran: true,
            ok: true,
            line: None,
            ms: Some(0),
            tail: None,
        }
    );
    assert_eq!(
        SetupRun::Failed {
            line: "x".to_owned(),
            exit: None,
            ms: 0,
            tail: String::new(),
        }
        .view(),
        SetupView {
            ran: true,
            ok: false,
            line: Some("x".to_owned()),
            ms: Some(0),
            tail: None,
        }
    );
}

#[test]
fn failed_is_true_only_for_failed() {
    assert!(!SetupRun::Skipped.failed());
    assert!(!SetupRun::Ok {
        ms: 0,
        tail: String::new(),
    }
    .failed());
    assert!(SetupRun::Failed {
        line: "x".to_owned(),
        exit: None,
        ms: 0,
        tail: String::new(),
    }
    .failed());
}

#[test]
fn exec_runs_at_the_resolved_size() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let c = cfg(&["a"], &[]);
    run_setup(&r.cx, &r.handle, &r.workdir, d.path(), &c);
    let (_, spec, _) = only(r.driver.execs());
    assert!(spec.output.is_none());
    assert_eq!(spec.class.as_deref(), Some(c.size.value.as_str()));
}

#[test]
fn interrupted_line_fails_and_stops_the_run() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results([Ok(ExecOutcome::Interrupted)])
        .copy_text("moor setup: $ a\n");
    let r = rig(d.path(), driver);
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
            line: "a".to_owned(),
            exit: None,
            ms: 0,
            tail: "moor setup: $ a".to_owned(),
        }
    );
    assert_eq!(r.driver.execs().len(), 1);
    assert_eq!(r.driver.copies().len(), 1);
    let warnings = r.cx.take_warnings();
    assert_eq!(warnings.len(), 1);
    let w = warnings.first().unwrap();
    assert_eq!(w.code, "setup_failed");
    assert!(w.message.contains("interrupted"));
}

#[test]
fn duration_is_measured_with_the_clock_on_success_and_failure() {
    let d = tempfile::tempdir().unwrap();
    for (code, ok) in [(0, true), (1, false)] {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
        let r = rig_on(
            d.path(),
            FakeDriver::new().exec_results(exits(&[code])),
            Arc::clone(&clock),
        );
        r.driver
            .on_exec(move || clock.sleep(Duration::from_millis(1500)));
        let got = run_setup(&r.cx, &r.handle, &r.workdir, d.path(), &cfg(&["a"], &[]));
        assert_eq!(got.ms(), Some(1500));
        assert_eq!(got.failed(), !ok);
        assert_eq!(got.view().ms, Some(1500));
    }
}

#[test]
fn skipped_setup_has_no_duration() {
    assert_eq!(SetupRun::Skipped.ms(), None);
}
