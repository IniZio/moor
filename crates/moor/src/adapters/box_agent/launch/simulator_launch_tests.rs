//! A launch on macOS creates and deletes the Task's simulator; a Linux launch never touches one.

use super::*;
use crate::adapters::box_agent::nono_release::fake::{
    release_for, tarball, FakeFetch, READY_BINARY,
};

const SIM_UDID: &str = "699BC2BD-E0B4-49A1-BFA1-3F7248BCD10D";

/// A proxy launch of Task `42` on a Box with `uname` `platform`, whose simulator script wrote
/// `SIM_UDID`.
fn launch_numbered(platform: &str, os: &'static str) -> (Arc<FakeDriver>, String) {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(READY_BINARY))));
    let release = release_for(&fetch, &tarball(READY_BINARY), (os, "arm64"));
    let d = driver(FakeDriver::new());
    let cred = CRED.replace("t1", "42");
    d.set_peek_text("box-1", &BoxPath::under(&cred, "uname").unwrap(), platform);
    let udid = BoxPath::under(&cred, "simulator-udid").unwrap();
    d.set_peek_text("box-1", &udid, SIM_UDID);
    let s = surface(FakeSurface::new("/r"));
    let a = BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        Arc::new(release),
    );
    let req = LaunchReq {
        task: TaskName::parse("42").unwrap(),
        ..req_with_proxy(&s)
    };
    a.launch(&req).unwrap();
    (d, script_of(&pane_line(&s)))
}

fn simctl_execs(d: &FakeDriver) -> usize {
    let argv = d.execs().into_iter().flat_map(|e| e.1.argv);
    argv.filter(|a| a.contains("simctl")).count()
}

#[test]
fn a_macos_launch_deletes_its_simulator_at_exit_and_makes_the_proof_dir() {
    let (d, script) = launch_numbered("Darwin arm64", "Darwin");
    let trap = script.lines().find(|l| l.starts_with("trap ")).unwrap();
    let delete = format!("xcrun simctl delete '\\''{SIM_UDID}'\\'''");
    assert!(
        trap.contains("rm -rf") && trap.contains(&delete),
        "{script}"
    );
    assert!(script.contains("'/work/box-1/.moor/proof'"), "{script}");
    assert_eq!(simctl_execs(&d), 1);
}

#[test]
fn a_linux_launch_never_runs_simctl() {
    let (d, script) = launch_numbered("Linux arm64", "Linux");
    assert!(!script.contains("simctl"), "{script}");
    assert_eq!(simctl_execs(&d), 0);
}
