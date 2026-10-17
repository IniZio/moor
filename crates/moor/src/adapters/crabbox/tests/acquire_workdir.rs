//! Tests for which working directory acquire reports.

use std::sync::Arc;

use moor_core::name::BoxPath;
use moor_core::name::{BoxHandle, ProviderId};
use moor_core::seam::driver::{Driver, DriverError, Platform};
use moor_core::seam::proc::Tool;

use super::{fixture, probe, req, Crabbox, Reply, ScriptedProc, PROBE_DIR};

#[test]
fn acquire_without_workroot_still_gets_workdir_from_probe() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_1 slug=-\n".into()),
    );
    probe(&p);
    let got = c.acquire(&req("ssh", None)).unwrap();
    assert_eq!(got.workdir.as_str(), PROBE_DIR);
    assert!(p
        .calls()
        .iter()
        .all(|call| call.args.first().map(String::as_str) != Some("inspect")));
}

fn run_calls(p: &ScriptedProc) -> usize {
    p.calls()
        .iter()
        .filter(|c| c.args.first().map(String::as_str) == Some("run"))
        .count()
}

fn acquire_with_ready(ready: &str) -> (Arc<ScriptedProc>, BoxPath) {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok(format!("leased cbx_1 slug=-\n{ready}\n")),
    );
    probe(&p);
    let got = c.acquire(&req("ssh", None)).unwrap();
    (p, got.workdir)
}

#[test]
fn acquire_uses_workdir_from_warmup_without_probing() {
    let (p, dir) =
        acquire_with_ready("ready ssh=u@h:22 workroot=/root/crabbox workdir=/root/crabbox/w/app");
    assert_ne!(PROBE_DIR, "/root/crabbox/w/app");
    assert_eq!(dir.as_str(), "/root/crabbox/w/app");
    assert_eq!(run_calls(&p), 0);
}

#[test]
fn acquire_probes_when_warmup_workdir_is_relative_or_empty() {
    for ready in [
        "ready ssh=u@h:22 workdir=app",
        "ready ssh=u@h:22 workdir=",
        "ready ssh=u@h:22 workroot=/r",
    ] {
        let (p, dir) = acquire_with_ready(ready);
        assert_eq!(dir.as_str(), PROBE_DIR, "{ready}");
        assert_eq!(run_calls(&p), 1, "{ready}");
    }
}

const MAC_ROOT: &str = "/Users/Shared/moor-u";

fn mac(root: Option<&str>) -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    let c = Crabbox::fake(p.clone()).with_target(Some("macos".into()), root.map(str::to_owned));
    (p, c)
}

fn mac_probe(p: &ScriptedProc, workdir: &str, home: &str) {
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok(format!("MOOR_WORKDIR={workdir}\nMOOR_HOME={home}\n")),
    );
}

#[test]
fn linux_acquire_reports_linux_and_does_not_probe() {
    let (p, dir) = acquire_with_ready("ready ssh=u@h:22 workdir=/root/w/app");
    assert_eq!(dir.as_str(), "/root/w/app");
    let (p2, c) = fixture();
    p2.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_1 slug=-\nready workdir=/w/a\n".into()),
    );
    assert_eq!(
        c.acquire(&req("ssh", None)).unwrap().platform,
        Platform::Linux
    );
    assert_eq!(run_calls(&p), 0);
    assert_eq!(run_calls(&p2), 0);
}

#[test]
fn mac_acquire_runs_crabbox_with_the_task_work_dir() {
    let (p, c) = mac(Some(MAC_ROOT));
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_1 slug=-\nready workdir=/Users/Shared/moor-u/t7/work/app\n".into()),
    );
    mac_probe(&p, "/Users/Shared/moor-u/t7/work/app", "/Users/u");
    let got = c.acquire(&req("ssh", None)).unwrap();
    assert_eq!(got.platform, Platform::Macos);
    assert_eq!(got.workdir.as_str(), "/Users/Shared/moor-u/t7/work/app");
    let warmup = p.calls().first().unwrap().clone();
    assert!(warmup.env.contains(&(
        "CRABBOX_STATIC_WORK_ROOT".into(),
        "/Users/Shared/moor-u/t7/work".into()
    )));
}

#[test]
fn mac_acquire_is_refused_and_released_when_the_root_is_in_the_login_home() {
    let (p, c) = mac(Some("/Users/u/moor"));
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_1 slug=-\n".into()),
    );
    p.on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()));
    mac_probe(&p, "/Users/u/moor/t7/work/app", "/Users/u");
    let err = c.acquire(&req("ssh", None)).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
    assert!(p
        .calls()
        .iter()
        .any(|call| call.args.first().map(String::as_str) == Some("stop")));
}

#[test]
fn mac_acquire_without_a_work_root_names_the_variable() {
    let (p, c) = mac(None);
    let err = c.acquire(&req("ssh", None)).unwrap_err();
    assert!(
        err.to_string().contains("CRABBOX_STATIC_WORK_ROOT"),
        "{err}"
    );
    assert!(p.calls().is_empty());
}

#[test]
fn listing_carries_the_platform() {
    let (p, c) = mac(Some(MAC_ROOT));
    p.on(
        Tool::Crabbox,
        &["list"],
        Reply::Ok(r#"[{"name":"a"}]"#.into()),
    );
    let got = c.list(&ProviderId::new("aws")).unwrap();
    assert_eq!(got.first().unwrap().platform, Platform::Macos);
    assert_eq!(got.first().unwrap().handle, BoxHandle::new("aws/a"));
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["list"],
        Reply::Ok(r#"[{"name":"a"}]"#.into()),
    );
    assert_eq!(
        c.list(&ProviderId::new("aws"))
            .unwrap()
            .first()
            .unwrap()
            .platform,
        Platform::Linux
    );
}
