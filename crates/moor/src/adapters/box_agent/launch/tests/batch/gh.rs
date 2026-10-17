//! The pinned `gh` reaches the Box the way `nono` does, only for a Proxy with a forge rule, and
//! a `gh` that cannot be delivered is a warning, not a failed launch.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::engine::{Engine, EngineError, LaunchReq, SessionRef};

use super::{agent, driver, exited, ready_release, req, DriverError};
use super::{FakeDriver, FakeSurface, NonoRelease};
use crate::adapters::box_agent::launch::gh;
use crate::adapters::box_agent::nono_release::fake::{ready_release_with_gh, READY_GH};

const GH_PUT: &str = "/home/box/.local/bin/gh";

fn forge_req(s: &FakeSurface, forge: bool) -> LaunchReq {
    let base = req(s, true);
    let addr = base.proxy.as_deref().expect("a proxy");
    let addr = ProxyAddr {
        box_port: addr.box_port,
        nonce: moor_core::Secret::new(addr.nonce.expose().to_vec()),
        ca_pem: addr.ca_pem.clone(),
        box_env: Vec::new(),
        forge,
    };
    LaunchReq {
        proxy: Some(Arc::new(addr)),
        ..base
    }
}

struct Run {
    d: Arc<FakeDriver>,
    s: Arc<FakeSurface>,
    out: Result<SessionRef, EngineError>,
}

fn run(d: FakeDriver, release: Arc<NonoRelease>, forge: Option<bool>) -> Run {
    let d = driver(d);
    let s = Arc::new(FakeSurface::new("/r"));
    let r = forge.map_or_else(|| req(&s, false), |forge| forge_req(&s, forge));
    let out = agent(&d, &s, release).launch(&r);
    Run { d, s, out }
}

/// Platform, agent and nono checks pass, then the gh check exits `gh_code`.
fn with_gh_check(gh_code: i32) -> FakeDriver {
    FakeDriver::new().exec_results(exited(&[0, 0, 0, gh_code]))
}

fn gh_puts(d: &FakeDriver) -> Vec<(String, Vec<u8>, u32)> {
    let puts = d.puts();
    let at = puts.iter().filter(|p| p.1.as_str() == GH_PUT);
    at.map(|p| (p.1.as_str().to_owned(), p.2.clone(), p.3))
        .collect()
}

fn assert_launched_with_warning(run: &Run) {
    let session = run.out.as_ref().expect("the launch goes on");
    let codes: Vec<_> = session.warnings.iter().map(|w| w.code.as_str()).collect();
    assert_eq!(codes, ["gh_not_delivered"]);
    assert_eq!(run.s.opened().len(), 1);
}

#[test]
fn a_missing_gh_is_uploaded_executable_for_a_forge_proxy() {
    let run = run(
        with_gh_check(gh::NOT_INSTALLED),
        ready_release_with_gh(),
        Some(true),
    );
    let session = run.out.as_ref().expect("launched");
    assert!(session.warnings.is_empty(), "{:?}", session.warnings);
    assert_eq!(run.s.opened().len(), 1);
    assert_eq!(
        gh_puts(&run.d),
        [(GH_PUT.to_owned(), READY_GH.to_vec(), 0o755)]
    );
}

#[test]
fn the_brief_still_lands_after_a_gh_upload() {
    let run = run(
        with_gh_check(gh::NOT_INSTALLED),
        ready_release_with_gh(),
        Some(true),
    );
    run.out.expect("launched");
    let puts = run.d.puts();
    assert!(puts.iter().any(|p| p.1.as_str().ends_with("brief.md")));
}

#[test]
fn an_installed_gh_is_not_uploaded() {
    let run = run(with_gh_check(0), ready_release_with_gh(), Some(true));
    let session = run.out.expect("launched");
    assert!(session.warnings.is_empty());
    assert!(gh_puts(&run.d).is_empty());
}

#[test]
fn a_gh_the_host_cannot_fetch_is_a_warning_and_the_agent_launches() {
    let run = run(FakeDriver::new(), ready_release(), Some(true));
    assert_launched_with_warning(&run);
    assert!(gh_puts(&run.d).is_empty());
}

#[test]
fn a_failed_gh_upload_is_a_warning_and_the_agent_launches() {
    let d = with_gh_check(gh::NOT_INSTALLED);
    let release = ready_release_with_gh();
    let run = run_failing_gh_put(d, release);
    assert_launched_with_warning(&run);
    assert!(run
        .d
        .puts()
        .iter()
        .any(|p| p.1.as_str().ends_with("brief.md")));
}

fn run_failing_gh_put(d: FakeDriver, release: Arc<NonoRelease>) -> Run {
    let d = driver(d);
    let s = Arc::new(FakeSurface::new("/r"));
    let (hook_d, seen) = (d.clone(), AtomicUsize::new(0));
    d.on_exec(move || {
        // The fourth exec is the gh check; the next put is then the gh upload.
        if seen.fetch_add(1, Ordering::SeqCst) == 3 {
            hook_d.set_fail_put(DriverError::Quota("upload broke".to_owned()));
        }
    });
    let out = agent(&d, &s, release).launch(&forge_req(&s, true));
    Run { d, s, out }
}

#[test]
fn a_check_that_fails_otherwise_is_a_warning_and_the_agent_launches() {
    let run = run(
        with_gh_check(gh::NOT_INSTALLED + 90),
        ready_release_with_gh(),
        Some(true),
    );
    assert_launched_with_warning(&run);
    assert!(run
        .d
        .puts()
        .iter()
        .any(|p| p.1.as_str().ends_with("brief.md")));
}

#[test]
fn a_proxy_without_a_forge_rule_neither_checks_nor_fetches_gh() {
    let run = run(FakeDriver::new(), ready_release(), Some(false));
    let session = run.out.expect("launched");
    assert!(session.warnings.is_empty());
    assert!(gh_puts(&run.d).is_empty());
    let scripts: Vec<_> = run
        .d
        .execs()
        .iter()
        .filter_map(|e| e.1.argv.get(2).cloned())
        .collect();
    assert!(
        scripts.iter().all(|s| !s.contains(gh::GH_PATH)),
        "{scripts:?}"
    );
}

#[test]
fn no_proxy_means_no_gh_either() {
    let run = run(FakeDriver::new(), ready_release(), None);
    let session = run.out.expect("launched");
    assert!(session.warnings.is_empty());
    assert!(gh_puts(&run.d).is_empty());
}
