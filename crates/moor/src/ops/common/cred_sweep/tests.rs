//! Tests for the credential sweep.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::box_key::CRED_SWEEP_SCRIPT;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::{BoxName, ProviderId};

use super::sweep;
use crate::cx::Cx;
use crate::store::TaskRow;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    row: TaskRow,
}

/// A Task with a Box that has a handle; Ready when `ready`.
fn fixture(driver: FakeDriver, ready: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
        Path::new("/home/me/proj"),
        driver.clone(),
        Arc::new(FakeSurface::new(PathBuf::from("/s"))),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set("/home/me/wt/t".to_owned()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            provider,
            task: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired { handle: got.handle });
    store.apply(d).unwrap();
    if !ready {
        store
            .apply(Decision::new(id).box_input(BoxInput::Sleep))
            .unwrap();
    }
    let row = store.task(id).unwrap().unwrap();
    Fx {
        _dir: dir,
        cx,
        driver,
        row,
    }
}

#[test]
fn ready_box_gets_one_sweep_exec() {
    let f = fixture(FakeDriver::new(), true);
    sweep(&f.cx, &f.row);
    let sweeps = f.driver.sweeps();
    assert_eq!(sweeps.len(), 1);
    assert!(f.driver.execs().is_empty());
    let spec = &sweeps.first().unwrap().1;
    assert_eq!(
        spec.argv.get(..2),
        Some(["sh".to_owned(), "-c".to_owned()].as_slice())
    );
    let script = spec.argv.last().unwrap();
    assert!(script.contains(".moor-cred-"));
    assert!(script.contains("/proc/"));
    assert_eq!(spec.host_cwd.as_deref(), Some(Path::new("/home/me/wt/t")));
}

#[test]
fn box_that_is_not_ready_gets_no_exec() {
    let f = fixture(FakeDriver::new(), false);
    sweep(&f.cx, &f.row);
    assert!(f.driver.sweeps().is_empty());
}

#[test]
fn row_without_a_box_gets_no_exec() {
    let mut f = fixture(FakeDriver::new(), true);
    f.row.boxed = None;
    sweep(&f.cx, &f.row);
    assert!(f.driver.sweeps().is_empty());
}

#[test]
fn timed_out_exec_is_ignored() {
    let driver = FakeDriver::new().fail_sweep(DriverError::Timeout(Duration::from_secs(15)));
    let f = fixture(driver, true);
    sweep(&f.cx, &f.row);
    assert_eq!(f.driver.sweeps().len(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn script_keeps_live_and_non_cred_dirs_and_removes_stale() {
    use std::fs;
    use std::process::Command;

    let home = tempfile::tempdir().unwrap();
    let live = home.path().join(".moor-cred-1-aaa");
    let stale = home.path().join(".moor-cred-1-bbb");
    let other = home.path().join("keep-me");
    for d in [&live, &stale, &other] {
        fs::create_dir(d).unwrap();
        fs::write(d.join("secret"), "x").unwrap();
    }
    let mut child = Command::new("sleep")
        .arg("30")
        .env("CRED_DIR", &live)
        .spawn()
        .unwrap();
    let out = Command::new("sh")
        .args(["-c", CRED_SWEEP_SCRIPT])
        .env("HOME", home.path())
        .output()
        .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(live.join("secret").exists());
    assert!(other.join("secret").exists());
    assert!(!stale.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn script_keeps_live_dir_when_home_has_trailing_slash() {
    use std::ffi::OsString;
    use std::fs;
    use std::process::Command;

    let home = tempfile::tempdir().unwrap();
    let live = home.path().join(".moor-cred-1-aaa");
    let stale = home.path().join(".moor-cred-1-bbb");
    for d in [&live, &stale] {
        fs::create_dir(d).unwrap();
        fs::write(d.join("secret"), "x").unwrap();
    }
    let mut slashed = OsString::from(home.path());
    slashed.push("/");
    let mut child = Command::new("sleep")
        .arg("30")
        .env("CRED_DIR", &live)
        .spawn()
        .unwrap();
    let out = Command::new("sh")
        .args(["-c", CRED_SWEEP_SCRIPT])
        .env("HOME", slashed)
        .output()
        .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(live.join("secret").exists());
    assert!(!stale.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn script_does_nothing_without_home() {
    use std::process::Command;

    let out = Command::new("sh")
        .args(["-c", CRED_SWEEP_SCRIPT])
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(out.status.success());
}
