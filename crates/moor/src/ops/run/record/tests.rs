//! Tests for `run --record`.

use moor_core::boxscript::record::{head_script, record_script, HEAD_FILE, HEAD_MOVED};
use moor_core::boxscript::{command_line, quote};
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::{BoxPath, ErrorKind, GateReq, RunReq};

use crate::error::CommandError;
use crate::ops::run::run;
use crate::ops::run::tests::{
    command, git_checkout, ran_events, ready_repo, req, Fx, LEADER_KILLED, SHA,
};
use crate::testing::driver::FakeDriver;

/// The Box HEAD a probe result names; the fixture puts the real HEAD of the Box checkout there.
const BOX_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn record_req(argv: &[&str]) -> RunReq {
    let mut r = req(argv);
    r.record = Some("ci".into());
    r
}

/// The Box workdir every fixture Box gets.
const WORKDIR: &str = "/work/cbx_1/repo";

/// A Ready Box with a git checkout whose probe writes `result` to the head file, with
/// [`BOX_SHA`] standing for the checkout's real HEAD; the probe exits 0.
fn probed(result: Option<&str>) -> Fx {
    probed_ending(ExecOutcome::Exited { code: 0 }, result)
}

fn probed_ending(probe: ExecOutcome, result: Option<&str>) -> Fx {
    probed_in(probe, result, None)
}

/// Like [`probed_ending`], for a Box whose stored `.moor.yaml` is `yaml`.
fn probed_in(probe: ExecOutcome, result: Option<&str>, yaml: Option<&str>) -> Fx {
    let f = ready_repo(FakeDriver::new().exec_results([Ok(probe)]), yaml);
    let head = git_checkout(&f);
    if let Some(text) = result {
        put(&f, HEAD_FILE, &text.replace(BOX_SHA, &head));
    }
    f
}

/// The real HEAD of the fixture Box's checkout.
fn box_sha(f: &Fx) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(f.box_dir.path().join("work"))
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// Leaves `text` at `rel` under the Box workdir.
fn put(f: &Fx, rel: &str, text: &str) {
    let path = BoxPath::under(WORKDIR, rel).unwrap();
    f.driver.set_peek_text("box-1", &path, text);
}

/// A Ready Box whose checkout is clean at its HEAD.
fn clean() -> Fx {
    probed(Some(&format!("head={BOX_SHA} dirty=0\n")))
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
    let head = box_sha(&f);
    let v = run(&f.cx, &record_req(&["true"])).unwrap();
    assert_eq!(v.sha.as_deref(), Some(head.as_str()));
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(
        row.sha.as_ref().map(moor_core::Sha::as_str),
        Some(head.as_str())
    );
    assert_ne!(head, SHA);
}

#[test]
fn a_dirty_box_checkout_is_refused_and_stores_no_run() {
    let f = probed(Some(&format!("head={BOX_SHA} dirty=2\n")));
    let msg = refused(&f, ErrorKind::Refused);
    assert_eq!(
        msg,
        "the Box checkout has uncommitted changes; commit them first"
    );
}

#[test]
fn one_uncommitted_file_is_refused() {
    let f = probed(Some(&format!("head={BOX_SHA} dirty=1\n")));
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
        let f = probed(result);
        let msg = refused(&f, ErrorKind::Failed);
        assert!(msg.starts_with("cannot read the Box HEAD"), "{msg}");
    }
}

#[test]
fn record_run_wraps_the_command_in_the_record_script() {
    let f = clean();
    let head = box_sha(&f);
    let argv = ["test", "-e", "tracked"];
    let v = run(&f.cx, &record_req(&argv)).unwrap();
    assert_eq!(v.exit, Some(0));
    let (_, spec) = f.driver.launches().remove(0);
    let line = command_line(&argv.map(str::to_owned));
    assert_eq!(
        command(&spec.argv),
        ["sh", "-c", &record_script(&[], &line, &head)]
    );
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.record.as_deref(), Some("ci"));
    assert_eq!(row.argv, argv);
    assert_eq!(row.exit, Some(0));
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn repo_setup_lines_precede_the_command() {
    let f = probed_in(
        ExecOutcome::Exited { code: 0 },
        Some(&format!("head={BOX_SHA} dirty=0\n")),
        Some("setup: ['make deps']\n"),
    );
    let head = box_sha(&f);
    run(&f.cx, &record_req(&["make", "test"])).unwrap();
    let (_, spec) = f.driver.launches().remove(0);
    let line = command_line(&["make".to_owned(), "test".to_owned()]);
    let setup = ["make deps".to_owned()];
    assert_eq!(
        command(&spec.argv),
        ["sh", "-c", &record_script(&setup, &line, &head)]
    );
}

#[test]
fn failed_prep_is_recorded_with_its_exit() {
    let f = probed_in(
        ExecOutcome::Exited { code: 0 },
        Some(&format!("head={BOX_SHA} dirty=0\n")),
        Some("setup: ['exit 3']\n"),
    );
    let v = run(&f.cx, &record_req(&["true"])).unwrap();
    assert_eq!(v.exit, Some(3));
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(row.exit, Some(3));
    assert!(!row.interrupted);
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn interrupted_record_run_has_no_exit() {
    let f = clean();
    let v = run(&f.cx, &record_req(&[LEADER_KILLED])).unwrap();
    assert!(v.interrupted && v.ran);
    assert_eq!(v.exit, None);
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.interrupted);
    assert_eq!(row.exit, None);
}

#[test]
fn passing_record_run_passes_the_gate_at_the_box_head() {
    let f = clean();
    let head = box_sha(&f);
    run(&f.cx, &record_req(&["true"])).unwrap();
    assert!(gate(&f, &head).is_ok());
    assert!(gate(&f, SHA).is_err());
}

#[test]
fn failing_record_run_fails_the_gate() {
    let f = clean();
    let head = box_sha(&f);
    run(&f.cx, &record_req(&["false"])).unwrap();
    let e = gate(&f, &head).unwrap_err();
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
    let f = clean();
    let head = box_sha(&f);
    let v = run(&f.cx, &record_req(&["echo two >> tracked"])).unwrap();
    let log = String::from_utf8_lossy(&f.host_log(v.run.unwrap())).into_owned();
    assert_ne!(v.exit, Some(0));
    assert_eq!(ran_error(&f), HEAD_MOVED, "{log}");
    assert!(gate(&f, &head).is_err());
}

#[test]
fn a_failing_record_run_without_the_moved_file_has_no_error() {
    let f = clean();
    let v = run(&f.cx, &record_req(&["false"])).unwrap();
    assert_eq!(v.exit, Some(1));
    assert_eq!(ran_error(&f), serde_json::Value::Null);
}
