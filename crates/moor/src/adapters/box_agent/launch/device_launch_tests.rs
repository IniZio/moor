//! A launch on macOS provisions and tears down the Task's devices; a Linux launch never touches one.

use super::*;
use crate::adapters::box_agent::device::DeviceKind;
use crate::adapters::box_agent::nono_release::fake::{
    release_for, tarball, FakeFetch, READY_BINARY,
};

const SIM_UDID: &str = "699BC2BD-E0B4-49A1-BFA1-3F7248BCD10D";

/// A proxy launch of Task `42` on a Box with `uname` `platform`, whose simulator script wrote
/// `SIM_UDID`.
fn launch_numbered(platform: &str, os: &'static str) -> (Arc<FakeDriver>, String) {
    let (d, s, out) = launch_with_kinds(platform, os, FakeDriver::new(), None);
    out.unwrap();
    (d, script_of(&pane_line(&s)))
}

/// Launches Task `42` like `launch_numbered`, on `driver`, with `kinds` replacing the enabled
/// device kinds when given.
fn launch_with_kinds(
    platform: &str,
    os: &'static str,
    driver_fake: FakeDriver,
    kinds: Option<&'static [&'static dyn DeviceKind]>,
) -> (Arc<FakeDriver>, Arc<FakeSurface>, Result<(), EngineError>) {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(READY_BINARY))));
    let release = release_for(&fetch, &tarball(READY_BINARY), (os, "arm64"));
    let d = driver(driver_fake);
    let cred = CRED.replace("t1", "42");
    d.set_peek_text("box-1", &BoxPath::under(&cred, "uname").unwrap(), platform);
    let udid = BoxPath::under(&cred, "simulator-udid").unwrap();
    d.set_peek_text("box-1", &udid, SIM_UDID);
    let s = surface(FakeSurface::new("/r"));
    let mut a = BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        Arc::new(release),
    );
    if let Some(kinds) = kinds {
        a = a.with_devices(kinds);
    }
    let req = LaunchReq {
        task: TaskName::parse("42").unwrap(),
        ..req_with_proxy(&s)
    };
    let out = a.launch(&req).map(|_| ());
    (d, s, out)
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

/// A kind a Darwin Box can host whose device never comes up.
struct Failing;

impl DeviceKind for Failing {
    fn available(&self, os: &str) -> bool {
        os == "Darwin"
    }
    fn provision(&self, _: &dyn Driver, _: &LaunchReq, _: &BoxPath) -> Option<String> {
        None
    }
    fn env(&self, _: &str) -> Vec<(String, String)> {
        vec![("FAILING_ENV".to_owned(), "failing-env".to_owned())]
    }
    fn grants(&self, _: &str, _: &str) -> Vec<String> {
        vec!["/failing-grant".to_owned()]
    }
    fn brief(&self, _: &str) -> String {
        "failing-brief".to_owned()
    }
    fn proof(&self, _: &str) -> String {
        "failing-proof".to_owned()
    }
    fn teardown(&self, _: &str) -> Option<String> {
        Some("echo failing-down".to_owned())
    }
    fn release_line(&self, _: &str) -> String {
        "true".to_owned()
    }
}

/// A kind a Darwin Box can host whose device is `fake-1`.
struct Fake;

impl DeviceKind for Fake {
    fn available(&self, os: &str) -> bool {
        os == "Darwin"
    }
    fn provision(&self, _: &dyn Driver, _: &LaunchReq, _: &BoxPath) -> Option<String> {
        Some("fake-1".to_owned())
    }
    fn env(&self, id: &str) -> Vec<(String, String)> {
        vec![("FAKE_ENV".to_owned(), id.to_owned())]
    }
    fn grants(&self, id: &str, _: &str) -> Vec<String> {
        vec![format!("/fake-grant/{id}")]
    }
    fn brief(&self, _: &str) -> String {
        "fake-brief".to_owned()
    }
    fn proof(&self, _: &str) -> String {
        "fake-proof".to_owned()
    }
    fn teardown(&self, id: &str) -> Option<String> {
        Some(format!("echo fake-down {id}"))
    }
    fn release_line(&self, _: &str) -> String {
        "true".to_owned()
    }
}

#[test]
fn a_kind_that_fails_to_provision_is_skipped_and_the_next_kind_still_runs() {
    let (d, s, out) = launch_with_kinds(
        "Darwin arm64",
        "Darwin",
        FakeDriver::new(),
        Some(&[&Failing, &Fake]),
    );
    out.unwrap();
    let script = script_of(&pane_line(&s));
    let trap = script.lines().find(|l| l.starts_with("trap ")).unwrap();
    assert!(trap.contains("echo fake-down fake-1"), "{script}");
    assert!(!script.contains("failing"), "{script}");
    assert!(!script.contains("Failing"), "{script}");
    assert_eq!(simctl_execs(&d), 0);
}

#[test]
fn a_failed_deliver_tears_down_every_provisioned_device() {
    let (d, _, out) = launch_with_kinds(
        "Darwin arm64",
        "Darwin",
        FakeDriver::new().fail_put(DriverError::NotFound),
        Some(&[&Failing, &Fake]),
    );
    assert!(out.is_err());
    let argv: Vec<_> = d.execs().into_iter().flat_map(|e| e.1.argv).collect();
    assert!(
        argv.iter().any(|a| a == "echo fake-down fake-1"),
        "{argv:?}"
    );
    assert!(!argv.iter().any(|a| a.contains("failing")), "{argv:?}");
}
