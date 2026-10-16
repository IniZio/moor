//! Tests for the setup runner.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::boxscript::setup::setup_script;
use moor_core::config::{Flags, TaskSettings, UserConfig};
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::SetupOutcome;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::view::SetupView;
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use super::{run_setup, setup_deadline, SetupRun, SETUP_LINE_DEADLINE};
use crate::cx::Cx;
use crate::testing::clock::FakeClock;
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
    rig_on(dir, driver, Arc::new(FakeClock::new(Ts::from_millis(1000))))
}

fn rig_on(dir: &Path, driver: FakeDriver, clock: Arc<FakeClock>) -> Rig {
    let driver = Arc::new(driver);
    let cx = Cx::fake_with_clock(
        dir,
        dir,
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new(dir)),
        Arc::new(ScriptedProc::new()),
        UserConfig::default(),
        clock,
    );
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: TaskId::new(1),
            name: TaskName::parse("demo").unwrap(),
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
        SyncMode::Full,
    );
    assert_eq!(got, SetupRun::Ok { ms: 0 });
    assert!(r.driver.copies().is_empty());
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn lines_run_as_one_generated_script_under_sh() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let c = cfg(&["one", "two", "three"], &[]);
    run_setup(&r.cx, &r.handle, &r.workdir, d.path(), &c, SyncMode::Full);
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
        SyncMode::Full,
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
        SyncMode::Full,
    );
    let (_, _, dl) = only(r.driver.execs());
    assert_eq!(dl, Deadline(SETUP_LINE_DEADLINE * 3));
    assert_eq!(SETUP_LINE_DEADLINE.as_secs(), 1800);
    assert_eq!(setup_deadline(0), Duration::ZERO);
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
        SyncMode::Full,
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "bad".to_owned(),
            exit: Some(3),
            ms: 0,
        }
    );
    assert_eq!(r.driver.execs().len(), 1);
    let (h, from, to, host_cwd) = only(r.driver.copies());
    assert_eq!(h, r.handle);
    assert_eq!(
        from.as_str(),
        format!("{}/.moor/setup.log", r.workdir.as_str())
    );
    assert_eq!(to, d.path().join(".moor/setup.log"));
    assert_eq!(host_cwd, d.path());
    let w = only(r.cx.take_warnings());
    assert_eq!(w.code, "setup_failed");
    assert!(w.message.contains("bad"), "{}", w.message);
    assert!(w.message.contains("exit 3"), "{}", w.message);
}

#[test]
fn the_warning_names_the_box_log_and_ends_with_its_tail() {
    let d = tempfile::tempdir().unwrap();
    let body: Vec<String> = (1..=30).map(|i| format!("out {i}")).collect();
    let log = format!("moor setup: $ bad\n{}\n", body.join("\n"));
    let driver = FakeDriver::new().exec_results(exits(&[2])).copy_text(&log);
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["bad"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    let path = format!("{}/.moor/setup.log", r.workdir.as_str());
    assert!(w.message.contains(&path), "{}", w.message);
    assert!(w.message.contains("out 30"), "{}", w.message);
    assert!(w.message.contains("out 11"), "{}", w.message);
    assert!(!w.message.contains("out 10\n"), "{}", w.message);
}

#[test]
fn a_log_with_invalid_utf8_still_finds_the_failing_line_and_tail() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .copy_text(b"moor setup: $ a\nmoor setup: $ b\nbad \xff byte\n");
    let r = rig(d.path(), driver);
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a", "b", "c"], &[]),
        SyncMode::Full,
    );
    assert_eq!(got.view().line.as_deref(), Some("b"));
    let w = only(r.cx.take_warnings());
    assert!(w.message.contains("bad"), "{}", w.message);
}

#[test]
fn a_redrawn_line_shows_only_its_final_frame() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .copy_text("moor setup: $ a\n10%\r50%\r100%\n");
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    assert!(w.message.ends_with("\n100%"), "{}", w.message);
    assert!(!w.message.contains("50%"), "{}", w.message);
    assert!(!w.message.contains('\r'), "{}", w.message);
}

#[test]
fn a_progress_line_does_not_push_earlier_context_out_of_the_tail() {
    let d = tempfile::tempdir().unwrap();
    let body: Vec<String> = (1..=19).map(|i| format!("out {i}")).collect();
    let log = format!("moor setup: $ a\n{}\n1%\r2%\r3%\r4%\r5%\n", body.join("\n"));
    let driver = FakeDriver::new().exec_results(exits(&[1])).copy_text(log);
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    assert!(w.message.contains("last 20 lines"), "{}", w.message);
    assert!(w.message.contains("out 1\n"), "{}", w.message);
    assert!(w.message.ends_with("\n5%"), "{}", w.message);
}

#[test]
fn long_tail_lines_are_capped() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .copy_text(format!("moor setup: $ a\n{}\n", "x".repeat(1000)));
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    let last = w.message.lines().last().unwrap();
    assert_eq!(last.chars().count(), 300);
    assert!(last.ends_with('…'), "{last}");
}

#[test]
fn control_characters_are_stripped_but_tabs_stay() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .copy_text("moor setup: $ a\n\u{1b}[31mred\u{1b}[0m\u{1b}]0;title\u{7}\tok\u{7}\n");
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    assert!(w.message.ends_with("\nred\tok"), "{}", w.message);
    assert!(!w.message.contains('\u{1b}'), "{}", w.message);
}

#[test]
fn an_unreadable_log_still_gets_its_path_in_the_warning() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .fail_copy_out(DriverError::NotFound);
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    let path = format!("{}/.moor/setup.log", r.workdir.as_str());
    assert!(w.message.contains(&path), "{}", w.message);
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
        SyncMode::Full,
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "b".to_owned(),
            exit: Some(1),
            ms: 0,
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
        SyncMode::Full,
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "a".to_owned(),
            exit: Some(1),
            ms: 0,
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
        SyncMode::Full,
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "slow".to_owned(),
            exit: None,
            ms: 0,
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
fn empty_setup_still_syncs_the_checkout_with_one_exec() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&[], &[("A", "1")]),
        SyncMode::Full,
    );
    assert_eq!(got, SetupRun::Skipped);
    let (_, spec, _) = only(r.driver.execs());
    assert_eq!(spec.sync, SyncMode::Full);
    assert_eq!(spec.argv, vec!["true".to_owned()]);
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn outcome_maps_skipped_and_ok_to_ok() {
    assert_eq!(SetupRun::Skipped.outcome(), SetupOutcome::Ok);
    assert_eq!(SetupRun::Ok { ms: 0 }.outcome(), SetupOutcome::Ok);
    assert_eq!(
        SetupRun::Failed {
            line: "x".to_owned(),
            exit: None,
            ms: 0,
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
        SetupRun::Ok { ms: 0 }.view(),
        SetupView {
            ran: true,
            ok: true,
            line: None,
            ms: Some(0)
        }
    );
    assert_eq!(
        SetupRun::Failed {
            line: "x".to_owned(),
            exit: None,
            ms: 0,
        }
        .view(),
        SetupView {
            ran: true,
            ok: false,
            line: Some("x".to_owned()),
            ms: Some(0)
        }
    );
}

#[test]
fn failed_is_true_only_for_failed() {
    assert!(!SetupRun::Skipped.failed());
    assert!(!SetupRun::Ok { ms: 0 }.failed());
    assert!(SetupRun::Failed {
        line: "x".to_owned(),
        exit: None,
        ms: 0,
    }
    .failed());
}

#[test]
fn exec_is_synced_from_the_worktree_at_the_resolved_size() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let c = cfg(&["a"], &[]);
    run_setup(&r.cx, &r.handle, &r.workdir, d.path(), &c, SyncMode::Full);
    let (_, spec, _) = only(r.driver.execs());
    assert_eq!(spec.sync, SyncMode::Full);
    assert!(spec.output.is_none());
    assert_eq!(spec.host_cwd.as_deref(), Some(d.path()));
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
        SyncMode::Full,
    );
    assert_eq!(
        got,
        SetupRun::Failed {
            line: "a".to_owned(),
            exit: None,
            ms: 0,
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
        let got = run_setup(
            &r.cx,
            &r.handle,
            &r.workdir,
            d.path(),
            &cfg(&["a"], &[]),
            SyncMode::Full,
        );
        assert_eq!(got.ms(), Some(1500));
        assert_eq!(got.failed(), !ok);
        assert_eq!(got.view().ms, Some(1500));
    }
}

#[test]
fn skipped_setup_has_no_duration() {
    assert_eq!(SetupRun::Skipped.ms(), None);
}
