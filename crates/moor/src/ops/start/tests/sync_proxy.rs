//! The Proxy starts while the checkout syncs, and each failure ends the start as a serial start would.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::seam::proc::Tool;
use moor_core::seam::proxy_host::{ProxyHostError, ProxyStatus};
use moor_core::BoxId;

use super::moor_yaml::script_base_yaml;
use super::setup::{exit, start_req};
use super::{world_at, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};

const YAML: &str = "setup: [a]\n";

/// A world over a repo with `setup: [a]` whose first execs give `results`, served by `host`.
fn world(
    results: impl IntoIterator<Item = Result<ExecOutcome, DriverError>>,
    host: &FakeProxyHost,
) -> (World, tempfile::TempDir) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), YAML).unwrap();
    let mut w = world_at(FakeDriver::new().exec_results(results), repo.path());
    w.cx = w.cx.with_proxy_host(Arc::new(host.clone()));
    script_base_yaml(&w, Some(YAML));
    (w, repo)
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

#[test]
fn the_proxy_is_ensured_while_the_sync_exec_runs() {
    let host = FakeProxyHost::new();
    let (w, _repo) = world([], &host);
    let (seen, first) = (Arc::new(AtomicBool::new(false)), AtomicBool::new(true));
    let (watch, flag) = (host.clone(), Arc::clone(&seen));
    w.driver.on_exec(move || {
        if !first.swap(false, Ordering::SeqCst) {
            return;
        }
        for _ in 0..1_000 {
            if ensured_stopped(&watch).0 {
                flag.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    run(&w.cx, &start_req()).unwrap();
    assert!(seen.load(Ordering::SeqCst), "{:?}", host.calls());
}

#[test]
fn a_failed_sync_ends_the_start_with_its_error_and_stops_the_proxy_it_started() {
    let host = FakeProxyHost::new();
    let (w, _repo) = world([Ok(exit(1))], &host);
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert!(
        err.error
            .to_string()
            .starts_with("syncing the checkout into the Box failed: exit 1"),
        "{err:?}"
    );
    assert_eq!(ensured_stopped(&host), (true, true));
    assert_eq!(lifecycle(&w), BoxLifecycle::SetupFailed);
}

#[test]
fn an_interrupted_sync_with_no_author_stops_the_proxy_it_started() {
    let host = FakeProxyHost::new();
    let (w, _repo) = world([Ok(ExecOutcome::Interrupted)], &host);
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert!(
        err.error
            .to_string()
            .starts_with("syncing the checkout into the Box failed: interrupted"),
        "{err:?}"
    );
    let (_, sync, _) = w.driver.execs().into_iter().next().unwrap();
    assert!(
        sync.output.is_some(),
        "a captured exec dies with moor on SIGINT"
    );
    assert_eq!(ensured_stopped(&host), (true, true));
}

#[test]
fn a_failed_sync_leaves_a_proxy_that_was_already_up() {
    let host = FakeProxyHost::new().with_status(ProxyStatus::Ready { box_port: 1 });
    let (w, _repo) = world([Ok(exit(1))], &host);
    run(&w.cx, &start_req()).unwrap_err();
    assert_eq!(ensured_stopped(&host), (true, false));
}

#[test]
fn a_failed_sync_wins_over_a_proxy_that_could_not_start() {
    let host = FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("boom".into()));
    let (w, _repo) = world([Ok(exit(1))], &host);
    let err = run(&w.cx, &start_req()).unwrap_err();
    let text = err.error.to_string();
    assert!(
        text.starts_with("syncing the checkout into the Box failed: exit 1"),
        "{text}"
    );
    assert!(!text.contains("boom"), "{text}");
    assert_eq!(ensured_stopped(&host), (true, false));
    let warnings = w.cx.take_warnings();
    assert!(
        warnings.iter().all(|x| !x.message.contains("boom")),
        "{warnings:?}"
    );
    assert_eq!(lifecycle(&w), BoxLifecycle::SetupFailed);
}

#[test]
fn a_proxy_that_cannot_start_while_the_sync_succeeds_fails_the_setup() {
    let host = FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("boom".into()));
    let (w, _repo) = world([Ok(exit(0))], &host);
    let v = run(&w.cx, &start_req()).unwrap();
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(v.setup.line.as_deref(), Some("the Proxy could not start"));
    let warnings = w.cx.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|x| x.code == "setup_failed" && x.message.contains("could not start: boom")),
        "{warnings:?}"
    );
    assert_eq!(ensured_stopped(&host), (true, false));
}

#[test]
fn a_failed_proxy_stop_after_a_failed_sync_is_a_warning() {
    let host = FakeProxyHost::new().with_stop_error(ProxyHostError::Incomplete("stuck".into()));
    let (w, _repo) = world([Ok(exit(1))], &host);
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert!(err.error.to_string().starts_with("syncing the checkout"));
    let warnings = w.cx.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|x| x.code == "proxy_stop_failed" && x.message.contains("stuck")),
        "{warnings:?}"
    );
}

#[test]
fn the_sync_log_is_owner_only_and_removed_when_the_sync_fails() {
    let host = FakeProxyHost::new();
    let (w, _repo) = world([Ok(exit(1))], &host);
    for (key, value) in [("user.name", "Ann Dev"), ("user.email", "a@x.io")] {
        w.proc.on(
            Tool::Git,
            &["config", "--get", key],
            Reply::Ok(format!("{value}\n")),
        );
    }
    let log = w.dir.path().join("sync").join("1.log");
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
