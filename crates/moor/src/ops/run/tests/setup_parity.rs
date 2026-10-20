//! A manual `moor run <Box> -- '<setup line>'` runs like setup: same env, same workdir.

use moor_core::config::Flags;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::ExecSpec;
use moor_core::BoxHandle;

use super::stage::command_execs;
use super::{command, ready, req, Fx};
use crate::ops::common::box_exec;
use crate::ops::common::facts::resolved;
use crate::ops::common::provision::moor_yaml;
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::setup::run_setup;
use crate::ops::run::run;

const YAML: &str = "setup: [make deps]\nenv:\n  B: \"2\"\n  A: \"1\"\n";

/// A Ready Box whose worktree and base commit both hold `.moor.yaml` as `YAML`.
fn with_env_yaml() -> Fx {
    let f = ready();
    std::fs::write(f.dir.path().join(".moor.yaml"), YAML).unwrap();
    f.proc.before(
        moor_core::seam::proc::Tool::Git,
        &["ls-tree"],
        crate::testing::proc::Reply::Ok(".moor.yaml\n".into()),
    );
    f.proc.before(
        moor_core::seam::proc::Tool::Git,
        &["show"],
        crate::testing::proc::Reply::Ok(YAML.into()),
    );
    f
}

/// The spec setup would exec for this Box's machine.
fn setup_spec(f: &Fx) -> ExecSpec {
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    let repo = moor_yaml::guard(&f.cx, &row, false).unwrap();
    let cfg = resolved(&f.cx, repo.as_ref(), &Flags::default());
    let boxed = row.boxed.unwrap();
    let workdir = recorded_workdir(boxed.workdir.as_deref().unwrap()).unwrap();
    let handle: BoxHandle = boxed.handle.unwrap();
    let before = command_execs(f).len();
    let proxy = box_exec::proxy(&f.cx, f.id, Some(handle.clone()), f.dir.path(), &cfg).ok();
    let provider = boxed.provider.as_str();
    let (worktree, sync) = (f.dir.path(), SyncMode::Full);
    run_setup(
        &f.cx,
        &handle,
        provider,
        &workdir,
        worktree,
        &cfg,
        sync,
        proxy.as_ref(),
    );
    command_execs(f).remove(before).1
}

/// The repo `env:` entries of the exec, apart from the Proxy variables.
fn repo_vars(spec: &ExecSpec) -> Vec<(&str, &str)> {
    spec.env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .filter(|(k, _)| matches!(*k, "A" | "B"))
        .collect()
}

#[test]
fn run_of_a_setup_line_has_the_env_and_cwd_setup_has() {
    let f = with_env_yaml();
    let setup = setup_spec(&f);
    run(&f.cx, &req(&["make deps"])).unwrap();
    let (_, spec, _) = command_execs(&f).remove(1);
    assert_eq!(command(&spec.argv), ["sh", "-c", "make deps"]);
    assert_eq!(repo_vars(&spec), [("A", "1"), ("B", "2")]);
    assert_eq!(spec.env, setup.env);
    assert_eq!(spec.cwd, setup.cwd);
    assert!(spec.cwd.is_some());
}

#[test]
fn recorded_run_has_the_repo_env_too() {
    let f = with_env_yaml();
    f.proc.on(
        moor_core::seam::proc::Tool::Git,
        &["status"],
        crate::testing::proc::Reply::Ok(String::new()),
    );
    let mut r = req(&["true"]);
    r.record = Some("ci".into());
    run(&f.cx, &r).unwrap();
    let (_, spec, _) = command_execs(&f).remove(0);
    assert_eq!(repo_vars(&spec), [("A", "1"), ("B", "2")]);
}

#[test]
fn box_without_a_recorded_workdir_keeps_the_driver_default_cwd() {
    let f = ready();
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(Fields {
            box_workdir: Field::Clear,
            ..Fields::default()
        }))
        .unwrap();
    run(&f.cx, &req(&["true"])).unwrap();
    let (_, spec, _) = command_execs(&f).remove(0);
    assert_eq!(spec.cwd, None);
}
