//! Tests for learning a Box's working directory from the in-box probe.

use std::path::{Path, PathBuf};

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::Tool;

use super::{fail, fixture, req, Reply};

const LEASE_OUT: &str =
    "leased cbx_X slug=moor-t7-ab12cd provider=sprites server=1 type=t ip=1.2.3.4\n\
ready ssh=u@h:22 network=public workroot=/w\n";

const PROBE_SCRIPT: &str = super::PROBE_SCRIPT;

fn probe_argv() -> Vec<String> {
    [
        "run",
        "--provider",
        "sprites",
        "--id",
        "moor-t7-ab12cd",
        "--no-hydrate",
        "--no-sync",
        "--",
        "sh",
        "-c",
        PROBE_SCRIPT,
    ]
    .map(str::to_owned)
    .to_vec()
}

fn handle() -> BoxHandle {
    BoxHandle::new("sprites/moor-t7-ab12cd")
}

fn probe_with(reply: Reply) -> Result<String, DriverError> {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["run"], reply);
    c.workdir(&handle(), Path::new("/wt/app"))
        .map(|d| d.as_str().to_owned())
}

#[test]
fn acquire_learns_the_workdir_from_the_probe() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(LEASE_OUT.to_owned()));
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok("MOOR_WORKDIR=/w/cbx_X/app\nMOOR_UNAME=Darwin arm64\n".to_owned()),
    );
    let mut r = req("sprites", None);
    r.worktree = Some(PathBuf::from("/wt/app"));
    let got = c.acquire(&r).unwrap();
    assert_eq!(got.workdir.as_str(), "/w/cbx_X/app");
    assert_ne!(got.workdir.as_str(), "/w/moor-t7-ab12cd");
    let calls = p.calls();
    assert_eq!(calls.len(), 2);
    let probe = calls.get(1).unwrap();
    assert_eq!(probe.args, probe_argv());
    assert!(probe.args.iter().any(|a| a == "run"));
    assert!(probe.args.iter().any(|a| a == "--no-sync"));
    let id_at = probe.args.iter().position(|a| a == "--id").unwrap();
    assert_eq!(probe.args.get(id_at + 1).unwrap(), "moor-t7-ab12cd");
    assert_eq!(probe.cwd.as_deref(), Some(Path::new("/wt/app")));
}

#[test]
fn workdir_after_wake_learns_the_path_from_the_probe() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok("MOOR_WORKDIR=/w/cbx_Y/app".to_owned()),
    );
    let got = c.workdir(&handle(), Path::new("/wt/app")).unwrap();
    assert_eq!(got.as_str(), "/w/cbx_Y/app");
    let calls = p.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls.first().unwrap().args, probe_argv());
    assert_eq!(
        calls.first().unwrap().cwd.as_deref(),
        Some(Path::new("/wt/app"))
    );
}

#[test]
fn last_marker_line_wins() {
    let got = probe_with(Reply::Ok(
        "MOOR_WORKDIR=/old\nMOOR_WORKDIR=/w/cbx_Y/app\r\n".to_owned(),
    ))
    .unwrap();
    assert_eq!(got, "/w/cbx_Y/app");
}

#[test]
fn probe_nonzero_exit_fails() {
    let err = probe_with(fail(1, "boom")).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
}

#[test]
fn probe_without_marker_fails() {
    let err = probe_with(Reply::Ok("hello\n".to_owned())).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
    assert!(format!("{err:?}").contains("MOOR_WORKDIR"), "{err:?}");
}

#[test]
fn probe_with_relative_path_fails() {
    let err = probe_with(Reply::Ok("MOOR_WORKDIR=w/app\n".to_owned())).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
}

#[test]
fn acquire_fails_when_the_probe_fails() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(LEASE_OUT.to_owned()));
    p.on(Tool::Crabbox, &["run"], fail(1, "boom"));
    let mut r = req("sprites", None);
    r.worktree = Some(PathBuf::from("/wt/app"));
    let err = c.acquire(&r).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
}

#[test]
fn acquire_on_ssh_reports_the_darwin_platform() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(LEASE_OUT.to_owned()));
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok("MOOR_WORKDIR=/Users/me/crabbox/app\nMOOR_UNAME=Darwin arm64\n".to_owned()),
    );
    let got = c.acquire(&req("ssh", None)).unwrap();
    assert_eq!(got.workdir.as_str(), "/Users/me/crabbox/app");
    assert_eq!(got.platform.os, "Darwin");
    assert_eq!(got.platform.arch, "arm64");
}

#[test]
fn acquire_fails_when_the_probe_has_no_uname_line() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(LEASE_OUT.to_owned()));
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok("MOOR_WORKDIR=/w/cbx_X/app\n".to_owned()),
    );
    let err = c.acquire(&req("ssh", None)).unwrap_err();
    assert!(format!("{err:?}").contains("MOOR_UNAME"), "{err:?}");
}
