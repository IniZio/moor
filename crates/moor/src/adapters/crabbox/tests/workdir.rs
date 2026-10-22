//! Tests for learning a Box's working directory from the in-box probe.

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::Tool;

use super::{fixture, req, Reply};
use crate::testing::proc::ScriptedProc;

fn handle() -> BoxHandle {
    BoxHandle::new("sprites/moor-t7-ab12cd")
}

/// The batch frame of one Run that exited `code` and printed `body`.
fn frame(code: i32, body: &str) -> Reply {
    Reply::Ok(format!("R {code} {}\n{body}\n", body.len()))
}

fn probe_with(reply: Reply) -> (Result<String, DriverError>, std::sync::Arc<ScriptedProc>) {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok("ssh u@h\n".to_owned()));
    p.on(Tool::Ssh, &[], reply);
    let got = c.workdir(&handle()).map(|d| d.as_str().to_owned());
    (got, p)
}

#[test]
fn the_probe_runs_over_ssh_and_looks_for_the_box_named_dir() {
    let (got, p) = probe_with(frame(0, "MOOR_WORKDIR=/w/cbx_Y/moor-t7-ab12cd\n"));
    assert_eq!(got.unwrap(), "/w/cbx_Y/moor-t7-ab12cd");
    let calls = p.calls();
    let crabbox: Vec<_> = calls.iter().filter(|c| c.tool == Tool::Crabbox).collect();
    assert_eq!(crabbox.len(), 1, "only the target lookup");
    assert_eq!(crabbox.first().unwrap().args.first().unwrap(), "ssh");
    let ssh: Vec<_> = calls.iter().filter(|c| c.tool == Tool::Ssh).collect();
    assert_eq!(ssh.len(), 1);
    let remote = ssh.first().unwrap().args.last().unwrap();
    assert!(
        remote.contains("find ") && remote.contains("moor-t7-ab12cd"),
        "{remote}"
    );
}

#[test]
fn acquire_without_a_ready_line_learns_the_workdir_from_the_probe() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_X slug=moor-t7-ab12cd\n".to_owned()),
    );
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok("ssh u@h\n".to_owned()));
    p.on(Tool::Ssh, &[], frame(0, "MOOR_WORKDIR=/w/cbx_X/app\n"));
    let got = c.acquire(&req("sprites", None)).unwrap();
    assert_eq!(got.workdir.as_str(), "/w/cbx_X/app");
    assert!(p.calls().iter().all(|c| c.args.first().unwrap() != "run"));
}

#[test]
fn last_marker_line_wins() {
    let (got, _) = probe_with(frame(0, "MOOR_WORKDIR=/old\nMOOR_WORKDIR=/w/cbx_Y/app\r\n"));
    assert_eq!(got.unwrap(), "/w/cbx_Y/app");
}

#[test]
fn a_probe_that_finds_no_single_dir_fails() {
    let (got, _) = probe_with(frame(1, ""));
    let err = got.unwrap_err();
    assert!(err.to_string().contains("no single workdir"), "{err:?}");
}

#[test]
fn probe_without_marker_fails() {
    let (got, _) = probe_with(frame(0, "hello\n"));
    let err = got.unwrap_err();
    assert!(format!("{err:?}").contains("MOOR_WORKDIR"), "{err:?}");
}

#[test]
fn probe_with_relative_path_fails() {
    let (got, _) = probe_with(frame(0, "MOOR_WORKDIR=w/app\n"));
    assert!(matches!(got, Err(DriverError::Failed { .. })), "{got:?}");
}

#[test]
fn a_lost_connection_during_the_probe_is_a_transport_error() {
    let (got, _) = probe_with(Reply::Fail {
        code: 255,
        stdout: String::new(),
        stderr: String::new(),
    });
    assert!(matches!(got, Err(DriverError::Transport { .. })), "{got:?}");
}

#[test]
fn the_probe_script_finds_exactly_one_dir_named_after_the_box() {
    let root = tempfile::tempdir().unwrap();
    let script = super::super::workdir::find_script(root.path().to_str().unwrap(), "moor-7-x");
    let run = || {
        let out = std::process::Command::new("sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        (out.status.code(), String::from_utf8(out.stdout).unwrap())
    };
    assert_eq!(run().0, Some(1), "none yet");
    let dir = root.path().join("work/crabbox/cbx_1/moor-7-x");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        run(),
        (Some(0), format!("MOOR_WORKDIR={}\n", dir.display()))
    );
    std::fs::create_dir_all(root.path().join("other/cbx_2/moor-7-x")).unwrap();
    assert_eq!(run().0, Some(1), "two matches");
}
