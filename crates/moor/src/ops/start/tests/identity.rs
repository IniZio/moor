use std::path::Path;

use moor_core::boxscript::quote;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::TaskId;

use super::moor_yaml::script_base_yaml;
use super::setup::{
    argvs, exit, is_identity, is_setup_line, script_argv, start_as, start_req, start_with,
};
use super::World;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

/// Whether the identity script in `argv` sets global `key` to `value`.
fn sets(argv: &[String], key: &str, value: &str) -> bool {
    let line = format!("git config --global {key} {}", quote(value));
    argv.get(2)
        .is_some_and(|s| is_identity(argv) && s.contains(&line))
}

/// Replaces each identity exec that sets the test author with a marker argv.
fn identity_marked(argvs: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let (name, email) = ("Ann Dev", "a@x.io");
    argvs
        .into_iter()
        .map(|a| {
            if sets(&a, "user.name", name) && sets(&a, "user.email", email) {
                identity_marker()
            } else {
                a
            }
        })
        .collect()
}

fn identity_marker() -> Vec<String> {
    vec!["identity".to_owned()]
}

#[test]
fn identity_is_set_in_the_box_before_the_first_setup_line() {
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a, b]\n", FakeDriver::new());
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(
        identity_marked(argvs(&w)),
        [identity_marker(), script_argv(&["a", "b"])]
    );
}

#[test]
fn identity_is_set_again_when_the_box_is_re_provisioned() {
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", FakeDriver::new());
    let id = TaskId::new(v.task.id);
    let store = w.cx.store().unwrap();
    let wt = store.task(id).unwrap().unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [a]\n").unwrap();
    script_base_yaml(&w, Some("setup: [a]\n"));
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    run(&w.cx, &start_req()).unwrap();
    let configs = argvs(&w).iter().filter(|a| is_identity(a)).count();
    assert_eq!(configs, 2);
}

/// The execs issued after the first `skip` ones.
fn argvs_after(w: &World, skip: usize) -> Vec<Vec<String>> {
    argvs(w).split_off(skip)
}

#[test]
fn identity_is_set_before_the_re_run_after_a_failed_setup() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(1))]);
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", driver);
    let wt =
        w.cx.store()
            .unwrap()
            .task(TaskId::new(v.task.id))
            .unwrap()
            .unwrap()
            .worktree;
    let wt = wt.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [b]\n").unwrap();
    script_base_yaml(&w, Some("setup: [b]\n"));
    let before = argvs(&w).len();
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    let sh = |l: &str| script_argv(&[l]);
    assert_eq!(
        identity_marked(argvs_after(&w, before)),
        [identity_marker(), sh("b")]
    );
}

#[test]
fn identity_is_set_before_the_re_run_when_the_setup_owner_is_dead() {
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", FakeDriver::new());
    let id = TaskId::new(v.task.id);
    let wt = w.cx.store().unwrap().task(id).unwrap().unwrap().worktree;
    let wt = wt.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [b]\n").unwrap();
    script_base_yaml(&w, Some("setup: [b]\n"));
    let held = Decision::new(id).intent(IntentChange::Set(Intent::Setup { pid: 4242 }));
    w.cx.store().unwrap().apply(held).unwrap();
    w.proc.dead(4242);
    let before = argvs(&w).len();
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    let sh = |l: &str| script_argv(&[l]);
    assert_eq!(
        identity_marked(argvs_after(&w, before)),
        [identity_marker(), sh("b")]
    );
}

#[test]
fn only_global_user_name_and_email_are_written_besides_setup_lines() {
    let (w, _) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", FakeDriver::new());
    let others: Vec<_> = argvs(&w)
        .into_iter()
        .filter(|a| !is_setup_line(a))
        .collect();
    assert_eq!(identity_marked(others.clone()), [identity_marker()]);
    let script = others.first().and_then(|a| a.get(2)).unwrap();
    assert_eq!(script.matches("git config").count(), 2);
    assert_eq!(script.matches("--global").count(), 2);
    let joined = argvs(&w).concat().join(" ");
    assert!(!joined.contains(".gitconfig") && !joined.contains("hosts.yml"));
}

#[test]
fn identity_adds_exactly_one_batch_box_call_to_a_fresh_start() {
    let (without, _) = start_with("setup: [a]\n", FakeDriver::new());
    let (with, _) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", FakeDriver::new());
    let count = |w: &World| {
        w.driver
            .box_calls()
            .iter()
            .filter(|c| *c == "batch")
            .count()
    };
    assert_eq!(count(&with), count(&without) + 1);
    assert_eq!(argvs(&with).iter().filter(|a| is_identity(a)).count(), 1);
}
