use std::path::Path;

use moor_core::boxscript::quote;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::driver::ExecSpec;
use moor_core::BoxId;

use super::moor_yaml::script_base_yaml;
use super::setup::{argvs, script_argv, start_as, start_req, start_with};
use super::World;
use crate::ops::start::run;
use crate::testing::driver::{is_clone_exec, FakeDriver};

/// The clone execs, in order.
fn clones(w: &World) -> Vec<ExecSpec> {
    (w.driver.execs().into_iter())
        .map(|e| e.1)
        .filter(is_clone_exec)
        .collect()
}

/// The identity lines the clone exec runs first for the test author.
fn identity_lines(name: &str, email: &str) -> String {
    format!(
        "git config --global user.name {}; echo user.name=$?; \
         git config --global user.email {}; echo user.email=$?; ",
        quote(name),
        quote(email)
    )
}

/// The shell script of the first clone exec.
fn first_script(w: &World) -> String {
    clones(w).first().map(script).expect("a clone exec")
}

/// The shell script of a clone exec, its Proxy wrapper peeled.
fn script(spec: &ExecSpec) -> String {
    spec.argv.last().cloned().unwrap_or_default()
}

#[test]
fn identity_is_set_inside_the_clone_exec_before_the_clone() {
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a, b]\n", FakeDriver::new());
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(clones(&w).len(), 1);
    let text = first_script(&w);
    let (identity, clone) = text.split_at(identity_lines("Ann Dev", "a@x.io").len());
    assert_eq!(identity, identity_lines("Ann Dev", "a@x.io"));
    assert!(
        clone.starts_with("export GIT_TERMINAL_PROMPT=0\n"),
        "{clone}"
    );
    assert_eq!(argvs(&w), [script_argv(&["a", "b"])]);
}

#[test]
fn without_an_author_the_clone_exec_only_clones() {
    let (w, _) = start_with("setup: [a]\n", FakeDriver::new());
    let text = first_script(&w);
    assert!(text.starts_with("export GIT_TERMINAL_PROMPT=0\n"), "{text}");
    assert!(!text.contains("git config --global"), "{text}");
}

#[test]
fn identity_is_set_again_when_the_box_is_re_provisioned() {
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", FakeDriver::new());
    let id = BoxId::new(v.boxed.id);
    let store = w.cx.store().unwrap();
    let wt = store.box_row(id).unwrap().unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [a]\n").unwrap();
    script_base_yaml(&w, Some("setup: [a]\n"));
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    run(&w.cx, &start_req()).unwrap();
    let with_identity = clones(&w)
        .iter()
        .filter(|s| script(s).contains("git config --global"))
        .count();
    assert_eq!(with_identity, 2);
}

#[test]
fn only_global_user_name_and_email_are_written_besides_setup_lines() {
    let (w, _) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", FakeDriver::new());
    let script = first_script(&w);
    assert_eq!(script.matches("git config").count(), 2);
    assert_eq!(script.matches("--global").count(), 2);
    let joined = argvs(&w).concat().join(" ") + &script;
    assert!(!joined.contains(".gitconfig") && !joined.contains("hosts.yml"));
}

#[test]
fn identity_adds_no_box_call_to_a_fresh_start() {
    let (without, _) = start_with("setup: [a]\n", FakeDriver::new());
    let (with, _) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", FakeDriver::new());
    assert_eq!(with.driver.box_calls(), without.driver.box_calls());
    assert_eq!(with.driver.execs().len(), without.driver.execs().len());
}

#[test]
fn the_clone_output_log_is_removed_after_it_is_read() {
    let driver = FakeDriver::new().exec_stdout(["user.name=0\nuser.email=0\n"]);
    let (w, _) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", driver);
    let log = w.dir.path().join("clone").join("1.log");
    assert!(log.parent().unwrap().is_dir());
    assert!(!log.exists());
    assert!(w
        .cx
        .take_warnings()
        .iter()
        .all(|x| x.code != "identity_failed"));
}
