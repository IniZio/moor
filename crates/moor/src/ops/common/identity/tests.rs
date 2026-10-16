//! Tests for the git identity setter.

use std::path::Path;
use std::sync::Arc;

use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::{BoxHandle, ProviderId, TaskId, TaskName};

use super::{set_identity, IDENTITY_DEADLINE};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;
use moor_core::boxscript::quote;

struct Rig {
    cx: Cx,
    driver: Arc<FakeDriver>,
    proc: Arc<ScriptedProc>,
    handle: BoxHandle,
}

fn rig(dir: &Path, driver: FakeDriver) -> Rig {
    let driver = Arc::new(driver);
    let proc = Arc::new(ScriptedProc::new());
    let cx = Cx::fake(
        dir,
        dir,
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new(dir)),
        Arc::clone(&proc) as Arc<_>,
    );
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: TaskId::new(1),
            name: TaskName::parse("demo").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    Rig {
        cx,
        driver,
        proc,
        handle: got.handle,
    }
}

fn author(r: &Rig, name: Option<&str>, email: Option<&str>) {
    for (key, value) in [("user.name", name), ("user.email", email)] {
        let reply = value.map_or_else(
            || Reply::Fail {
                code: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
            |v| Reply::Ok(format!("{v}\n")),
        );
        r.proc.on(Tool::Git, &["config", "--get", key], reply);
    }
}

fn scripts(r: &Rig) -> Vec<String> {
    r.driver
        .execs()
        .into_iter()
        .map(|(_, s, _)| s.argv.last().cloned().unwrap_or_default())
        .collect()
}

#[test]
fn both_keys_are_set_in_one_box_call() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let before = r.driver.box_calls();
    author(&r, Some("Ann Dev"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    assert_eq!(
        r.driver.box_calls().get(before.len()..),
        Some(&["batch".to_owned()][..])
    );
    let scripts = scripts(&r);
    assert_eq!(scripts.len(), 1);
    let script = scripts.first().unwrap();
    let name = script
        .find("git config --global user.name 'Ann Dev'")
        .unwrap();
    let email = script
        .find("git config --global user.email 'a@x.io'")
        .unwrap();
    assert!(name < email);
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn only_email_set_runs_one_box_call_with_one_key() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let before = r.driver.box_calls();
    author(&r, None, Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    assert_eq!(
        r.driver.box_calls().get(before.len()..),
        Some(&["batch".to_owned()][..])
    );
    let script = scripts(&r).remove(0);
    assert!(script.contains("git config --global user.email 'a@x.io'"));
    assert!(!script.contains("user.name"));
}

#[test]
fn unset_author_runs_nothing() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let before = r.driver.box_calls();
    author(&r, None, None);
    set_identity(&r.cx, &r.handle, d.path());
    assert_eq!(r.driver.box_calls(), before);
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn not_a_repo_runs_nothing() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let before = r.driver.box_calls();
    r.proc.on(
        Tool::Git,
        &["config", "--get"],
        Reply::Fail {
            code: 128,
            stdout: String::new(),
            stderr: "fatal: not a git repository".to_owned(),
        },
    );
    set_identity(&r.cx, &r.handle, d.path());
    assert_eq!(r.driver.box_calls(), before);
}

#[test]
fn driver_error_warns_for_each_key() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Err(DriverError::NotFound)]);
    let r = rig(d.path(), driver);
    author(&r, Some("Ann Dev"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    let warnings = r.cx.take_warnings();
    assert_eq!(warnings.len(), 2);
    assert!(warnings.iter().all(|w| w.code == "identity_failed"));
    let err = DriverError::NotFound.to_string();
    assert_eq!(
        warnings.first().unwrap().message,
        format!("could not set git user.name in the box: {err}")
    );
    assert_eq!(
        warnings.get(1).unwrap().message,
        format!("could not set git user.email in the box: {err}")
    );
}

#[test]
fn per_key_exit_codes_from_stdout_warn_only_the_failing_key() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results([Ok(ExecOutcome::Exited { code: 3 })])
        .run_stdout("user.name=3\nuser.email=0\n");
    let r = rig(d.path(), driver);
    author(&r, Some("Ann Dev"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    let warnings = r.cx.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings.first().unwrap().message,
        "could not set git user.name in the box: exit 3"
    );
}

#[test]
fn exit_code_without_stdout_warns_for_each_key() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 1 })]);
    let r = rig(d.path(), driver);
    author(&r, Some("Ann Dev"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    let warnings = r.cx.take_warnings();
    assert_eq!(warnings.len(), 2);
    assert!(warnings.iter().all(|w| w.message.ends_with("exit 1")));
}

#[test]
fn values_are_shell_quoted_and_carry_no_config_files() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let hostile = "$(touch x); `id` it's";
    author(&r, Some(hostile), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    let execs = r.driver.execs();
    assert_eq!(execs.len(), 1);
    let (h, spec, dl) = execs.first().unwrap();
    assert_eq!(*h, r.handle);
    assert_eq!(*dl, Deadline(IDENTITY_DEADLINE * 2));
    assert!(spec.env.is_empty() && spec.cwd.is_none());
    assert!(spec.argv.starts_with(&["sh".into(), "-c".into()]));
    let script = spec.argv.last().unwrap();
    assert!(script.contains(&format!("user.name {}", quote(hostile))));
    assert!(!script.contains(".gitconfig") && !script.contains("hosts.yml"));
}

#[test]
fn interrupted_run_warns_for_each_key() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(ExecOutcome::Interrupted)]);
    let r = rig(d.path(), driver);
    author(&r, Some("Ann Dev"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    let warnings = r.cx.take_warnings();
    assert_eq!(warnings.len(), 2);
    assert!(warnings
        .iter()
        .all(|w| w.code == "identity_failed" && w.message.contains("interrupted")));
}
