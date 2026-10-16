//! Tests for `run --record`.

use moor_core::boxscript::{command_line, record::record_script};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::proc::Tool;
use moor_core::{ErrorKind, GateReq, RunReq};

use crate::error::CommandError;
use crate::ops::run::run;
use crate::ops::run::tests::{command, ran_events, ready, ready_with, req, Fx, SHA};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

fn record_req(argv: &[&str]) -> RunReq {
    let mut r = req(argv);
    r.record = Some("ci".into());
    r
}

/// A Ready Task whose worktree is clean.
fn clean() -> Fx {
    clean_with(FakeDriver::new())
}

fn clean_with(driver: FakeDriver) -> Fx {
    let f = ready_with(driver);
    f.proc.on(Tool::Git, &["status"], Reply::Ok(String::new()));
    f
}

/// The refusal message; nothing ran and nothing was recorded.
fn refused(f: &Fx, r: &RunReq, kind: ErrorKind) -> String {
    let e = run(&f.cx, r).unwrap_err();
    assert_eq!(e.kind, kind, "{e:?}");
    assert!(f.driver.execs().is_empty());
    assert!(f.driver.calls().iter().all(|c| !c.starts_with("exec")));
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
    e.error.to_string()
}

fn gate(f: &Fx) -> Result<crate::ops::gate::GateView, CommandError> {
    let r = GateReq {
        sha: SHA.into(),
        name: "ci".into(),
        command: None,
    };
    crate::ops::gate::run(&f.cx, &r)
}

#[test]
fn record_with_no_sync_is_usage() {
    let f = clean();
    let mut r = record_req(&["true"]);
    r.no_sync = true;
    let msg = refused(&f, &r, ErrorKind::Usage);
    assert_eq!(msg, "--record cannot be used with --no-sync");
}

#[test]
fn missing_worktree_is_refused() {
    let f = clean();
    let gone = f.dir.path().join("gone");
    let fields = Fields {
        worktree: Field::Set(gone.display().to_string()),
        ..Fields::default()
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(fields))
        .unwrap();
    let msg = refused(&f, &record_req(&["true"]), ErrorKind::Failed);
    assert_eq!(msg, format!("worktree {} is missing", gone.display()));
}

#[test]
fn untracked_files_are_refused() {
    let f = ready();
    f.proc
        .on(Tool::Git, &["status"], Reply::Ok("?? new.txt\n".into()));
    let msg = refused(&f, &record_req(&["true"]), ErrorKind::Refused);
    assert!(msg.contains("uncommitted changes"), "{msg}");
}

#[test]
fn modified_files_are_refused() {
    let f = ready();
    f.proc
        .on(Tool::Git, &["status"], Reply::Ok(" M a.rs\n".into()));
    let msg = refused(&f, &record_req(&["true"]), ErrorKind::Refused);
    assert!(msg.contains("uncommitted changes"), "{msg}");
}

#[test]
fn record_run_wraps_the_command_in_the_record_script() {
    let f = clean();
    let argv = ["make", "test"];
    let v = run(&f.cx, &record_req(&argv)).unwrap();
    assert_eq!(v.exit, Some(0));
    let (_, spec, _) = f.driver.execs().remove(0);
    let line = command_line(&argv.map(str::to_owned));
    assert_eq!(
        command(&spec.argv),
        ["sh", "-c", &record_script(&[], &line)]
    );
    assert_eq!(spec.sync, SyncMode::Full);
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.sha.as_ref().map(moor_core::Sha::as_str), Some(SHA));
    assert_eq!(row.record.as_deref(), Some("ci"));
    assert_eq!(row.argv, argv);
    assert_eq!(row.exit, Some(0));
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn repo_setup_lines_precede_the_command() {
    let f = clean();
    std::fs::write(f.dir.path().join(".moor.yaml"), "setup: ['make deps']\n").unwrap();
    f.proc
        .before(Tool::Git, &["ls-tree"], Reply::Ok(".moor.yaml\n".into()));
    f.proc.before(
        Tool::Git,
        &["show"],
        Reply::Ok("setup: ['make deps']\n".into()),
    );
    run(&f.cx, &record_req(&["make", "test"])).unwrap();
    let (_, spec, _) = f.driver.execs().remove(0);
    let line = command_line(&["make".to_owned(), "test".to_owned()]);
    let setup = ["make deps".to_owned()];
    assert_eq!(
        command(&spec.argv),
        ["sh", "-c", &record_script(&setup, &line)]
    );
}

#[test]
fn failed_prep_is_recorded_with_its_exit() {
    let f = clean_with(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 3 })]));
    let v = run(&f.cx, &record_req(&["true"])).unwrap();
    assert_eq!(v.exit, Some(3));
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.exit, Some(3));
    assert!(!row.interrupted);
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn interrupted_record_run_has_no_exit() {
    let f = clean_with(FakeDriver::new().exec_results([Ok(ExecOutcome::Interrupted)]));
    let v = run(&f.cx, &record_req(&["sleep", "9"])).unwrap();
    assert!(v.interrupted && v.ran);
    assert_eq!(v.exit, None);
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.interrupted);
    assert_eq!(row.exit, None);
}

#[test]
fn passing_record_run_passes_the_gate() {
    let f = clean();
    run(&f.cx, &record_req(&["true"])).unwrap();
    assert!(gate(&f).is_ok());
}

#[test]
fn failing_record_run_fails_the_gate() {
    let f = clean_with(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 1 })]));
    run(&f.cx, &record_req(&["false"])).unwrap();
    let e = gate(&f).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("gate `ci` has no passing run"));
}
