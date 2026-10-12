//! Tests for the git identity setter.

use std::path::Path;
use std::sync::Arc;

use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan, ExecOutcome, SyncMode};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::{BoxHandle, ProviderId, TaskId, TaskName};

use super::{set_identity, IDENTITY_DEADLINE};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

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
            egress: EgressPlan::Open,
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

fn argvs(r: &Rig) -> Vec<Vec<String>> {
    r.driver
        .execs()
        .into_iter()
        .map(|(_, s, _)| s.argv)
        .collect()
}

#[test]
fn both_keys_are_set_in_order_with_exact_argv() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    author(&r, Some("Ann Dev"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    assert_eq!(
        argvs(&r),
        vec![
            vec!["git", "config", "--global", "user.name", "Ann Dev"],
            vec!["git", "config", "--global", "user.email", "a@x.io"],
        ]
    );
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn only_email_set_runs_one_exec() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    author(&r, None, Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    assert_eq!(
        argvs(&r),
        vec![vec!["git", "config", "--global", "user.email", "a@x.io"]]
    );
}

#[test]
fn unset_author_runs_nothing() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    author(&r, None, None);
    set_identity(&r.cx, &r.handle, d.path());
    assert!(r.driver.execs().is_empty());
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn not_a_repo_runs_nothing() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
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
    assert!(r.driver.execs().is_empty());
}

#[test]
fn failing_exec_warns_and_the_email_is_still_attempted() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([
        Err(DriverError::NotFound),
        Ok(ExecOutcome::Exited { code: 1 }),
    ]);
    let r = rig(d.path(), driver);
    author(&r, Some("Ann Dev"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    assert_eq!(r.driver.execs().len(), 2);
    let warnings = r.cx.take_warnings();
    assert_eq!(warnings.len(), 2);
    assert!(warnings.iter().all(|w| w.code == "identity_failed"));
    assert!(warnings.iter().any(|w| w.message.contains("exit 1")));
}

#[test]
fn only_argv_values_are_sent_without_a_shell_or_config_files() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    author(&r, Some("$(touch x); `id`"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    let execs = r.driver.execs();
    assert_eq!(execs.len(), 2);
    assert!(execs
        .iter()
        .all(|(h, _, dl)| { *h == r.handle && *dl == Deadline(IDENTITY_DEADLINE) }));
    for (_, spec, _) in &execs {
        assert!(spec
            .argv
            .starts_with(&["git".into(), "config".into(), "--global".into()]));
        assert_eq!(spec.argv.len(), 5);
        assert!(spec.env.is_empty() && spec.cwd.is_none());
        assert!(!spec
            .argv
            .iter()
            .any(|a| a.contains(".gitconfig") || a.contains("hosts.yml") || a == "sh"));
    }
    let value = execs.first().and_then(|(_, s, _)| s.argv.last());
    assert_eq!(value.map(String::as_str), Some("$(touch x); `id`"));
}

#[test]
fn exec_neither_syncs_nor_redirects_output() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    author(&r, Some("Ann Dev"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    let execs = r.driver.execs();
    assert_eq!(execs.len(), 2);
    for (_, spec, _) in &execs {
        assert_eq!(spec.sync, SyncMode::None);
        assert!(spec.output.is_none());
    }
}

#[test]
fn interrupted_exec_warns_and_the_email_is_still_attempted() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(ExecOutcome::Interrupted)]);
    let r = rig(d.path(), driver);
    author(&r, Some("Ann Dev"), Some("a@x.io"));
    set_identity(&r.cx, &r.handle, d.path());
    assert_eq!(r.driver.execs().len(), 2);
    let warnings = r.cx.take_warnings();
    assert_eq!(warnings.len(), 1);
    let w = warnings.first().unwrap();
    assert_eq!(w.code, "identity_failed");
    assert!(w.message.contains("interrupted"));
}
