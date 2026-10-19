use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::proc::Tool;
use moor_core::BoxId;

use super::setup::{exit, failed_setup_with, is_identity, is_setup_line, setup_execs, start_req};
use super::{req, world_at, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

/// Starts `demo` with one setup line and a git author, so identity runs too.
fn start(driver: FakeDriver) -> (World, moor_core::view::StartView) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [a]\n").unwrap();
    let w = world_at(driver, repo.path());
    let ok = |v: &str| Reply::Ok(format!("{v}\n"));
    w.proc
        .on(Tool::Git, &["config", "--get", "user.name"], ok("Ann"));
    w.proc
        .on(Tool::Git, &["config", "--get", "user.email"], ok("a@x.io"));
    let v = run(&w.cx, &req("demo")).unwrap();
    (w, v)
}

/// The stored workdir and the cwd of every setup and identity exec.
fn observed(
    w: &World,
    v: &moor_core::view::StartView,
) -> (String, Vec<Option<String>>, Vec<Option<String>>) {
    let row = w.cx.store().unwrap().task(BoxId::new(v.boxed.id));
    let stored = row.unwrap().unwrap().boxed.unwrap().workdir.unwrap();
    let (mut setup, mut identity) = (Vec::new(), Vec::new());
    for (_, spec, _) in w.driver.execs() {
        let cwd = spec.cwd.map(|p| p.as_str().to_owned());
        if is_setup_line(&spec.argv) {
            setup.push(cwd);
        } else if is_identity(&spec.argv) {
            identity.push(cwd);
        }
    }
    (stored, setup, identity)
}

#[test]
fn setup_and_identity_run_in_the_probed_workdir_not_one_built_from_the_handle() {
    let (w, v) = start(FakeDriver::new());
    let (stored, setup, identity) = observed(&w, &v);
    let want = "/work/cbx_1/demo";
    assert_eq!(stored, want);
    assert!(!setup.is_empty());
    assert!(setup.iter().all(|c| c.as_deref() == Some(want)));
    // Identity sets global git config, which does not depend on cwd, so it
    // may run in crabbox's own workdir (no cwd).
    assert!(!identity.is_empty());
    assert!(identity
        .iter()
        .all(|c| c.is_none() || c.as_deref() == Some(want)));
    let handle_built = Some("/work/box-1".to_owned());
    assert!(!setup.contains(&handle_built) && !identity.contains(&handle_built));
}

#[test]
fn a_custom_work_root_is_the_root_of_the_probed_workdir() {
    let (w, v) = start(FakeDriver::new().work_root("/w"));
    let (stored, setup, _) = observed(&w, &v);
    assert_eq!(stored, "/w/cbx_1/demo");
    assert!(setup.iter().all(|c| c.as_deref() == Some("/w/cbx_1/demo")));
}

#[test]
fn every_box_exec_runs_from_the_task_worktree() {
    // crabbox derives its claim root from the host cwd, so a stray cwd gets the lease refused.
    let (w, v) = start(FakeDriver::new());
    let row = w.cx.store().unwrap().task(BoxId::new(v.boxed.id));
    let worktree = std::path::PathBuf::from(row.unwrap().unwrap().worktree.unwrap());
    let execs = w.driver.execs();
    assert!(execs.iter().any(|(_, s, _)| is_identity(&s.argv)));
    assert!(execs.iter().any(|(_, s, _)| is_setup_line(&s.argv)));
    for (_, spec, _) in &execs {
        assert_eq!(
            spec.host_cwd.as_deref(),
            Some(worktree.as_path()),
            "{:?}",
            spec.argv
        );
    }
}

#[test]
fn start_in_a_replaced_box_runs_setup_in_a_workdir_whose_root_holds_a_parent_component() {
    let driver = FakeDriver::new().work_root("/w/bin/../box").exec_results([
        Ok(exit(0)),
        Ok(exit(0)),
        Ok(exit(1)),
        Ok(exit(0)),
        Ok(exit(0)),
    ]);
    let (w, id) = failed_setup_with("setup: [a]\n", driver);
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    let execs = setup_execs(&w);
    assert_eq!(execs.len(), 2);
    let cwd = execs
        .last()
        .and_then(|e| e.1.cwd.as_ref())
        .map(|p| p.as_str().to_owned());
    assert_eq!(cwd.as_deref(), Some("/w/bin/../box/cbx_2/demo"));
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    assert_eq!(
        row.boxed.unwrap().workdir.as_deref(),
        Some("/w/bin/../box/cbx_2/demo")
    );
}
