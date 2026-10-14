use moor_core::seam::proc::Tool;
use moor_core::TaskId;

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
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    let stored = row.unwrap().unwrap().boxed.unwrap().workdir.unwrap();
    let (mut setup, mut identity) = (Vec::new(), Vec::new());
    for (_, spec, _) in w.driver.execs() {
        let cwd = spec.cwd.map(|p| p.as_str().to_owned());
        if spec.argv.first().map(String::as_str) == Some("sh") {
            setup.push(cwd);
        } else if spec.argv.starts_with(&["git".into(), "config".into()]) {
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
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    let worktree = std::path::PathBuf::from(row.unwrap().unwrap().worktree.unwrap());
    let execs = w.driver.execs();
    assert!(execs.iter().any(|(_, s, _)| s.argv[0] == "git"));
    assert!(execs.iter().any(|(_, s, _)| s.argv[0] == "sh"));
    for (_, spec, _) in &execs {
        assert_eq!(
            spec.host_cwd.as_deref(),
            Some(worktree.as_path()),
            "{:?}",
            spec.argv
        );
    }
}
