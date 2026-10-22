//! A manual `moor run <Box> -- '<setup line>'` runs like setup: same env, same workdir.

use moor_core::boxscript::record::HEAD_FILE;
use moor_core::boxscript::shell_argv;
use moor_core::config::Flags;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::driver::ExecSpec;
use moor_core::{BoxHandle, BoxPath};

use super::{command, ready, ready_yaml, req, Fx, SHA};
use crate::ops::common::box_exec;
use crate::ops::common::facts::{repo_config, resolved};
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::setup::run_setup;
use crate::ops::run::run;

const YAML: &str = "setup: [make deps]\nenv:\n  B: \"2\"\n  A: \"1\"\n";

/// A Ready Box whose stored `.moor.yaml` is `YAML`.
fn with_env_yaml() -> Fx {
    ready_yaml(YAML)
}

/// The spec setup would exec for this Box's machine.
fn setup_spec(f: &Fx) -> ExecSpec {
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    let repo = repo_config(&row).unwrap();
    let cfg = resolved(&f.cx, repo.as_ref(), &Flags::default());
    let boxed = row.boxed.unwrap();
    let workdir = recorded_workdir(boxed.workdir.as_deref().unwrap()).unwrap();
    let handle: BoxHandle = boxed.handle.unwrap();
    let before = f.driver.execs().len();
    let forge_repo = row.repo.as_ref().map(|r| &r.repo);
    let proxy = box_exec::proxy(&f.cx, f.id, Some(handle.clone()), forge_repo, &cfg).ok();
    let provider = boxed.provider.as_str();
    run_setup(
        &f.cx,
        &handle,
        provider,
        &workdir,
        f.dir.path(),
        &cfg,
        proxy.as_ref(),
    );
    f.driver.execs().remove(before).1
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
    let (_, spec, _) = f.driver.execs().remove(1);
    assert_eq!(command(&spec.argv), shell_argv("make deps"));
    assert_eq!(repo_vars(&spec), [("A", "1"), ("B", "2")]);
    assert_eq!(spec.env, setup.env);
    assert_eq!(spec.cwd, setup.cwd);
    assert!(spec.cwd.is_some());
}

#[test]
fn recorded_run_has_the_repo_env_too() {
    let f = with_env_yaml();
    let head = BoxPath::under("/work/cbx_1/repo", HEAD_FILE).unwrap();
    let clean = format!("head={SHA} dirty=0\n");
    f.driver.set_peek_text("box-1", &head, &clean);
    let mut r = req(&["true"]);
    r.record = Some("ci".into());
    run(&f.cx, &r).unwrap();
    let (_, spec, _) = f.driver.execs().remove(1);
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
    let (_, spec, _) = f.driver.execs().remove(0);
    assert_eq!(spec.cwd, None);
}
