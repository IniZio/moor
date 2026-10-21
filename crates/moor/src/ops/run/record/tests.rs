//! Tests for `run --record`.

use moor_core::boxscript::record::{head_script, record_script, HEAD_FILE, HEAD_MOVED, MOVED_FILE};
use moor_core::boxscript::{command_line, quote};
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::{BoxPath, ErrorKind, GateReq, RunReq};

use crate::error::CommandError;
use crate::ops::run::run;
use crate::ops::run::tests::{command, ran_events, ready_with, req, Fx, SHA};
use crate::testing::driver::FakeDriver;

/// The Box HEAD, which differs from the host HEAD the fixture's git answers.
const BOX_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn record_req(argv: &[&str]) -> RunReq {
    let mut r = req(argv);
    r.record = Some("ci".into());
    r
}

/// The Box workdir every fixture Box gets.
const WORKDIR: &str = "/work/cbx_1/repo";

/// A Ready Box whose probe writes `result` to the head file; the probe exits 0, then the run
/// gets `outcomes`.
fn probed(result: Option<&str>, outcomes: Vec<ExecOutcome>) -> Fx {
    probed_ending(ExecOutcome::Exited { code: 0 }, result, outcomes)
}

fn probed_ending(probe: ExecOutcome, result: Option<&str>, outcomes: Vec<ExecOutcome>) -> Fx {
    let probe = std::iter::once(Ok(probe));
    let driver = FakeDriver::new().exec_results(probe.chain(outcomes.into_iter().map(Ok)));
    let f = ready_with(driver);
    if let Some(text) = result {
        put(&f, HEAD_FILE, text);
    }
    f
}

/// Leaves `text` at `rel` under the Box workdir.
fn put(f: &Fx, rel: &str, text: &str) {
    let path = BoxPath::under(WORKDIR, rel).unwrap();
    f.driver.set_peek_text("box-1", &path, text);
}

/// A Ready Box whose checkout is clean at [`BOX_SHA`].
fn clean() -> Fx {
    clean_then(vec![])
}

fn clean_then(outcomes: Vec<ExecOutcome>) -> Fx {
    probed(Some(&format!("head={BOX_SHA} dirty=0\n")), outcomes)
}

/// The refusal message; only the head probe ran and nothing was recorded.
fn refused(f: &Fx, kind: ErrorKind) -> String {
    let e = run(&f.cx, &record_req(&["true"])).unwrap_err();
    assert_eq!(e.kind, kind, "{e:?}");
    let execs = f.driver.execs();
    assert_eq!(execs.len(), 1, "only the head probe runs: {execs:?}");
    let probe = |(_, spec, _): &(_, ExecSpec, _)| {
        spec.argv.last().is_some_and(|s| s.contains(&head_script()))
    };
    assert!(execs.iter().all(probe), "{execs:?}");
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
    e.error.to_string()
}

fn gate(f: &Fx, sha: &str) -> Result<crate::ops::gate::GateView, CommandError> {
    let r = GateReq {
        sha: sha.into(),
        name: "ci".into(),
        command: None,
    };
    crate::ops::gate::run(&f.cx, &r)
}

#[test]
fn a_record_run_is_stored_under_the_box_head() {
    let f = clean();
    let v = run(&f.cx, &record_req(&["true"])).unwrap();
    assert_eq!(v.sha.as_deref(), Some(BOX_SHA));
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.sha.as_ref().map(moor_core::Sha::as_str), Some(BOX_SHA));
    assert_ne!(BOX_SHA, SHA);
}

#[test]
fn a_dirty_box_checkout_is_refused_and_stores_no_run() {
    let f = probed(Some(&format!("head={BOX_SHA} dirty=2\n")), vec![]);
    let msg = refused(&f, ErrorKind::Refused);
    assert_eq!(
        msg,
        "the Box checkout has uncommitted changes; commit them first"
    );
}

#[test]
fn one_uncommitted_file_is_refused() {
    let f = probed(Some(&format!("head={BOX_SHA} dirty=1\n")), vec![]);
    assert_eq!(
        refused(&f, ErrorKind::Refused),
        "the Box checkout has uncommitted changes; commit them first"
    );
}

#[test]
fn a_failed_probe_fails_even_when_a_head_file_is_left() {
    let f = probed_ending(
        ExecOutcome::Exited { code: 1 },
        Some(&format!("head={BOX_SHA} dirty=0\n")),
        vec![],
    );
    let msg = refused(&f, ErrorKind::Failed);
    assert!(
        msg.starts_with("cannot read the Box HEAD: the probe ended"),
        "{msg}"
    );
}

#[test]
fn the_probe_runs_in_the_box_workdir() {
    let f = clean();
    run(&f.cx, &record_req(&["true"])).unwrap();
    let (_, spec, _) = f.driver.execs().remove(0);
    let script = format!("cd {} || exit 1\n{}", quote(WORKDIR), head_script());
    assert_eq!(spec.argv, ["sh", "-c", &script]);
}

#[test]
fn a_box_head_that_cannot_be_read_fails_and_stores_no_run() {
    for result in [None, Some("head=nope dirty=0\n")] {
        let f = probed(result, vec![]);
        let msg = refused(&f, ErrorKind::Failed);
        assert!(msg.starts_with("cannot read the Box HEAD"), "{msg}");
    }
}

#[test]
fn record_run_wraps_the_command_in_the_record_script() {
    let f = clean();
    let argv = ["make", "test"];
    let v = run(&f.cx, &record_req(&argv)).unwrap();
    assert_eq!(v.exit, Some(0));
    let (_, spec, _) = f.driver.execs().remove(1);
    let line = command_line(&argv.map(str::to_owned));
    assert_eq!(
        command(&spec.argv),
        ["sh", "-c", &record_script(&[], &line, BOX_SHA)]
    );
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.record.as_deref(), Some("ci"));
    assert_eq!(row.argv, argv);
    assert_eq!(row.exit, Some(0));
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn repo_setup_lines_precede_the_command() {
    let f = clean();
    std::fs::write(f.dir.path().join(".moor.yaml"), "setup: ['make deps']\n").unwrap();
    f.proc.before(
        moor_core::seam::proc::Tool::Git,
        &["ls-tree"],
        crate::testing::proc::Reply::Ok(".moor.yaml\n".into()),
    );
    f.proc.before(
        moor_core::seam::proc::Tool::Git,
        &["show"],
        crate::testing::proc::Reply::Ok("setup: ['make deps']\n".into()),
    );
    run(&f.cx, &record_req(&["make", "test"])).unwrap();
    let (_, spec, _) = f.driver.execs().remove(1);
    let line = command_line(&["make".to_owned(), "test".to_owned()]);
    let setup = ["make deps".to_owned()];
    assert_eq!(
        command(&spec.argv),
        ["sh", "-c", &record_script(&setup, &line, BOX_SHA)]
    );
}

#[test]
fn failed_prep_is_recorded_with_its_exit() {
    let f = clean_then(vec![ExecOutcome::Exited { code: 3 }]);
    let v = run(&f.cx, &record_req(&["true"])).unwrap();
    assert_eq!(v.exit, Some(3));
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.exit, Some(3));
    assert!(!row.interrupted);
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn interrupted_record_run_has_no_exit() {
    let f = clean_then(vec![ExecOutcome::Interrupted]);
    let v = run(&f.cx, &record_req(&["sleep", "9"])).unwrap();
    assert!(v.interrupted && v.ran);
    assert_eq!(v.exit, None);
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.interrupted);
    assert_eq!(row.exit, None);
}

#[test]
fn passing_record_run_passes_the_gate_at_the_box_head() {
    let f = clean();
    run(&f.cx, &record_req(&["true"])).unwrap();
    assert!(gate(&f, BOX_SHA).is_ok());
    assert!(gate(&f, SHA).is_err());
}

#[test]
fn failing_record_run_fails_the_gate() {
    let f = clean_then(vec![ExecOutcome::Exited { code: 1 }]);
    run(&f.cx, &record_req(&["false"])).unwrap();
    let e = gate(&f, BOX_SHA).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("gate `ci` has no passing run"));
}

/// The error the run's `ran` event carries.
fn ran_error(f: &Fx) -> serde_json::Value {
    let events = f.cx.store().unwrap().events(f.id, 1000).unwrap();
    let ran = events.iter().rev().find(|e| e.kind == "ran").unwrap();
    ran.payload
        .get("error")
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

#[test]
fn a_record_run_whose_checkout_moved_is_stored_failed_as_moved() {
    let f = clean_then(vec![ExecOutcome::Exited { code: 1 }]);
    put(&f, MOVED_FILE, "");
    let v = run(&f.cx, &record_req(&["true"])).unwrap();
    assert_eq!(v.exit, Some(1));
    assert_eq!(ran_error(&f), HEAD_MOVED);
    assert!(gate(&f, BOX_SHA).is_err());
}

#[test]
fn a_failing_record_run_without_the_moved_file_has_no_error() {
    let f = clean_then(vec![ExecOutcome::Exited { code: 1 }]);
    run(&f.cx, &record_req(&["false"])).unwrap();
    assert_eq!(ran_error(&f), serde_json::Value::Null);
}
