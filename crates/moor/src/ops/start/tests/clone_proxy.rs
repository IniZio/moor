//! The Proxy is ready before the repo clones in the Box, and each failure ends the start.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proxy_host::{ProxyHostError, ProxyStatus};
use moor_core::BoxId;

use super::setup::{exit, start_req};
use super::{req, script_base_yaml, World};
use crate::ops::start::run;
use crate::testing::driver::{is_clone_exec, FakeDriver};
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall, FAKE_BOX_PORT};

const YAML: &str = "setup: [a]\n";

/// A world over a GitHub repo with `.moor.yaml` `yaml` whose first execs give `results`, served by `host`.
fn world_with(
    yaml: &str,
    results: impl IntoIterator<Item = Result<ExecOutcome, DriverError>>,
    host: &FakeProxyHost,
) -> World {
    let mut w = super::world(FakeDriver::new().exec_results(results));
    w.cx = w.cx.with_proxy_host(Arc::new(host.clone()));
    script_base_yaml(&w, Some(yaml));
    w
}

fn world(
    results: impl IntoIterator<Item = Result<ExecOutcome, DriverError>>,
    host: &FakeProxyHost,
) -> World {
    world_with(YAML, results, host)
}

/// Whether the Proxy was ensured, and then stopped.
fn ensured_stopped(host: &FakeProxyHost) -> (bool, bool) {
    let calls = host.calls();
    let ensured = calls.iter().any(|c| matches!(c, ProxyHostCall::Ensure(_)));
    let stopped = calls.iter().any(|c| matches!(c, ProxyHostCall::Stop(_)));
    (ensured, stopped)
}

fn lifecycle(w: &World) -> BoxLifecycle {
    let store = w.cx.store().unwrap();
    store.box_row(BoxId::new(1)).unwrap().unwrap().lifecycle
}

fn clone_exec(w: &World) -> ExecSpec {
    let mut execs = w.driver.execs().into_iter().map(|e| e.1);
    execs.find(is_clone_exec).expect("a clone exec")
}

#[test]
fn the_proxy_is_ready_before_the_clone_exec_and_its_env_is_on_it() {
    let host = FakeProxyHost::new();
    let w = world_with("", [], &host);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (watch, record) = (host.clone(), Arc::clone(&seen));
    w.driver
        .on_exec(move || record.lock().unwrap().push(ensured_stopped(&watch).0));
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(*seen.lock().unwrap(), [true], "{:?}", host.calls());
    let spec = clone_exec(&w);
    let script = spec.argv.get(2).cloned().unwrap_or_default();
    assert!(
        script.contains(&format!(
            "export HTTPS_PROXY=\"http://127.0.0.1:{FAKE_BOX_PORT}\""
        )),
        "{script}"
    );
    let clone = spec.argv.last().unwrap();
    assert!(
        clone.contains(
            "git clone -q --filter=blob:none --no-checkout -- 'https://github.com/o/n.git' \"$tmp\""
        ),
        "{clone}"
    );
    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(1))
            .unwrap()
            .unwrap();
    let branch = row.branch.unwrap();
    assert!(clone.contains(&format!("-b '{branch}'")), "{clone}");
    let kinds = super::setup::kinds(&w, BoxId::new(1));
    assert!(kinds.contains(&"cloned".to_owned()), "{kinds:?}");
}

#[test]
fn a_box_with_no_repo_starts_no_proxy_and_runs_no_clone() {
    let host = FakeProxyHost::new();
    let mut w = super::world(FakeDriver::new());
    w.cx = w.cx.with_proxy_host(Arc::new(host.clone()));
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(ensured_stopped(&host), (false, false));
    assert!(w.driver.execs().is_empty(), "{:?}", w.driver.execs());
    let kinds = super::setup::kinds(&w, BoxId::new(1));
    assert!(!kinds.contains(&"cloned".to_owned()), "{kinds:?}");
}

#[test]
fn a_failed_clone_ends_the_start_with_its_step_and_stops_the_proxy_it_started() {
    let host = FakeProxyHost::new();
    let w = world([Ok(exit(71))], &host);
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert!(
        err.error.to_string().starts_with(
            "cloning the repo into the Box failed: base abc123 is not on https://github.com/o/n.git; push it first"
        ),
        "{err:?}"
    );
    assert_eq!(ensured_stopped(&host), (true, true));
    assert_eq!(lifecycle(&w), BoxLifecycle::SetupFailed);
    assert_eq!(w.driver.execs().len(), 1, "setup does not run");
}

#[test]
fn an_interrupted_clone_stops_the_proxy_it_started() {
    let host = FakeProxyHost::new();
    let w = world([Ok(ExecOutcome::Interrupted)], &host);
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert!(
        err.error
            .to_string()
            .starts_with("cloning the repo into the Box failed: interrupted"),
        "{err:?}"
    );
    assert!(
        clone_exec(&w).output.is_some(),
        "a captured exec dies with moor on SIGINT"
    );
    assert_eq!(ensured_stopped(&host), (true, true));
}

#[test]
fn a_failed_clone_leaves_a_proxy_that_was_already_up() {
    let host = FakeProxyHost::new().with_status(ProxyStatus::Ready { box_port: 1 });
    let w = world([Ok(exit(1))], &host);
    run(&w.cx, &start_req()).unwrap_err();
    assert_eq!(ensured_stopped(&host), (true, false));
}

#[test]
fn a_proxy_that_cannot_start_fails_the_clone_before_any_exec() {
    let host = FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("boom".into()));
    let w = world([], &host);
    let err = run(&w.cx, &start_req()).unwrap_err();
    let text = err.error.to_string();
    assert!(
        text.starts_with("cloning the repo into the Box failed: the Proxy could not start: boom"),
        "{text}"
    );
    assert!(w.driver.execs().is_empty(), "{:?}", w.driver.execs());
    assert_eq!(ensured_stopped(&host), (true, false));
    assert_eq!(lifecycle(&w), BoxLifecycle::SetupFailed);
}

#[test]
fn a_failed_proxy_stop_after_a_failed_clone_is_a_warning() {
    let host = FakeProxyHost::new().with_stop_error(ProxyHostError::Incomplete("stuck".into()));
    let w = world([Ok(exit(1))], &host);
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert!(err.error.to_string().starts_with("cloning the repo"));
    let warnings = w.cx.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|x| x.code == "proxy_stop_failed" && x.message.contains("stuck")),
        "{warnings:?}"
    );
}

#[test]
fn the_clone_log_is_owner_only_and_removed_when_the_clone_fails() {
    let host = FakeProxyHost::new();
    let w = world([Ok(exit(1))], &host);
    let log = w.dir.path().join("clone").join("1.log");
    let (modes, path) = (Arc::new(Mutex::new(Vec::new())), log.clone());
    let seen = Arc::clone(&modes);
    w.driver.on_exec(move || {
        let mode = |p: &Path| std::fs::metadata(p).map(|m| m.permissions().mode() & 0o777);
        if let (Ok(file), Ok(dir)) = (mode(&path), mode(path.parent().unwrap())) {
            seen.lock().unwrap().push((file, dir));
        }
    });
    run(&w.cx, &start_req()).unwrap_err();
    assert_eq!(*modes.lock().unwrap(), [(0o600, 0o700)]);
    assert!(!log.exists());
}
