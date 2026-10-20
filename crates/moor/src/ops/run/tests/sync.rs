//! Tests for how `run` picks its sync mode.

use moor_core::boxscript::boxwork::{BOXWORK_FILE, BOXWORK_NO_CHECKOUT};
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::{BoxPath, ErrorKind, RunReq};

use super::{ran_events, ready, ready_with, req, run, Fx};
use crate::testing::driver::FakeDriver;

const USER_CMD: &str = "user-cmd";

fn probe_result(text: &str) -> FakeDriver {
    let path = BoxPath::under("/work/cbx_1/repo", BOXWORK_FILE).unwrap();
    FakeDriver::new().peek_text("box-1", &path, text)
}

fn sync_req() -> RunReq {
    let mut r = req(&[USER_CMD]);
    r.sync = true;
    r
}

fn user_execs(f: &Fx) -> usize {
    f.driver
        .execs()
        .iter()
        .filter(|(_, spec, _)| spec.argv.concat().contains(USER_CMD))
        .count()
}

fn with_session(f: &Fx) {
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "agent-1", None)
        .unwrap();
}

/// Runs `sync_req()` and asserts the refusal left no run row and ran no user command.
fn refused_by_probe(f: &Fx) -> Vec<String> {
    let e = run(&f.cx, &sync_req()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused, "{e:?}");
    assert_eq!(user_execs(f), 0);
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
    e.hints
}

#[test]
fn box_with_an_agent_session_is_not_synced() {
    let f = ready();
    with_session(&f);
    let v = run(&f.cx, &req(&[USER_CMD])).unwrap();
    assert_eq!(v.sync, SyncMode::None);
    let (_, spec, _) = f.driver.execs().remove(0);
    assert_eq!(spec.sync, SyncMode::None);
}

#[test]
fn box_without_an_agent_session_is_synced() {
    let f = ready();
    let v = run(&f.cx, &req(&[USER_CMD])).unwrap();
    assert_eq!(v.sync, SyncMode::Full);
    let (_, spec, _) = f.driver.execs().remove(0);
    assert_eq!(spec.sync, SyncMode::Full);
}

#[test]
fn no_sync_skips_the_sync_even_without_a_session() {
    let f = ready();
    let mut r = req(&[USER_CMD]);
    r.no_sync = true;
    assert_eq!(run(&f.cx, &r).unwrap().sync, SyncMode::None);
}

#[test]
fn sync_overrides_the_session_default_when_the_box_is_clean() {
    let f = ready_with(probe_result("ahead=0 dirty=0"));
    with_session(&f);
    let v = run(&f.cx, &sync_req()).unwrap();
    assert_eq!(v.sync, SyncMode::Full);
    assert_eq!(user_execs(&f), 1);
    let (_, spec, _) = f.driver.execs().pop().unwrap();
    assert_eq!(spec.sync, SyncMode::Full);
    assert_eq!(ran_events(&f), 1);
}

#[test]
fn sync_is_refused_when_the_box_holds_unsaved_work() {
    let f = ready_with(probe_result("ahead=1 dirty=2"));
    with_session(&f);
    let hints = refused_by_probe(&f);
    assert!(hints.iter().any(|h| h.contains("--no-sync")), "{hints:?}");
    assert!(hints.iter().any(|h| h.contains("git bundle")), "{hints:?}");
}

#[test]
fn sync_is_refused_when_the_probe_cannot_tell() {
    refused_by_probe(&ready());
    refused_by_probe(&ready_with(probe_result("nonsense")));
    let failing = probe_result("ahead=0 dirty=0").fail_peek(DriverError::Unavailable("x".into()));
    let hints = refused_by_probe(&ready_with(failing));
    assert!(hints.iter().any(|h| h.contains("--no-sync")), "{hints:?}");
}

#[test]
fn sync_is_refused_when_the_box_is_gone() {
    let gone = probe_result("ahead=0 dirty=0").fail_peek(DriverError::NotFound);
    let f = ready_with(gone);
    let e = run(&f.cx, &sync_req()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused, "{e:?}");
    assert!(e.error.to_string().contains("gone"), "{e:?}");
    assert_eq!(user_execs(&f), 0);
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
}

#[test]
fn sync_with_no_sync_is_usage() {
    let f = ready_with(probe_result("ahead=0 dirty=0"));
    let mut r = sync_req();
    r.no_sync = true;
    let e = run(&f.cx, &r).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage, "{e:?}");
    assert!(f.driver.execs().is_empty());
}

#[test]
fn sync_is_refused_when_the_box_has_no_git_checkout() {
    let code = BOXWORK_NO_CHECKOUT;
    let driver = probe_result("ahead=0 dirty=0").exec_results([Ok(ExecOutcome::Exited { code })]);
    let hints = refused_by_probe(&ready_with(driver));
    assert!(hints.iter().any(|h| h.contains("--no-sync")), "{hints:?}");
}
