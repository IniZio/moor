//! Tests for learning a Box's working directory from the in-box probe.

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::Tool;

use super::{fail, fixture, req, Reply};

const LEASE_OUT: &str =
    "leased cbx_X slug=moor-t7-ab12cd provider=sprites server=1 type=t ip=1.2.3.4\n\
ready ssh=u@h:22 network=public workroot=/w\n";

const PROBE_SCRIPT: &str = r#"printf "MOOR_WORKDIR=%s\n" "$(pwd)""#;

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
    c.workdir(&handle()).map(|d| d.as_str().to_owned())
}

#[test]
fn acquire_learns_the_workdir_from_the_probe() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(LEASE_OUT.to_owned()));
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok("MOOR_WORKDIR=/w/cbx_X/app\n".to_owned()),
    );
    let got = c.acquire(&req("sprites", None)).unwrap();
    assert_eq!(got.workdir.as_str(), "/w/cbx_X/app");
    assert_ne!(got.workdir.as_str(), "/w/moor-t7-ab12cd");
    let calls = p.calls();
    assert_eq!(calls.len(), 2);
    let probe = calls.get(1).unwrap();
    let want: Vec<String> = probe_argv()
        .into_iter()
        .map(|a| {
            if a == "moor-t7-ab12cd" {
                "moor-7-ab12cd".into()
            } else {
                a
            }
        })
        .collect();
    assert_eq!(probe.args, want);
    assert!(probe.args.iter().any(|a| a == "run"));
    assert!(probe.args.iter().any(|a| a == "--no-sync"));
    let id_at = probe.args.iter().position(|a| a == "--id").unwrap();
    assert_eq!(probe.args.get(id_at + 1).unwrap(), "moor-7-ab12cd");
}

#[test]
fn workdir_after_wake_learns_the_path_from_the_probe() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok("MOOR_WORKDIR=/w/cbx_Y/app".to_owned()),
    );
    let got = c.workdir(&handle()).unwrap();
    assert_eq!(got.as_str(), "/w/cbx_Y/app");
    let calls = p.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls.first().unwrap().args, probe_argv());
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
    let err = c.acquire(&req("sprites", None)).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
}
