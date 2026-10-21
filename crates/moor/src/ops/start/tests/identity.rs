use std::path::Path;

use moor_core::boxscript::quote;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::ExecSpec;
use moor_core::BoxId;

use super::moor_yaml::script_base_yaml;
use super::setup::{argvs, script_argv, start_as, start_req, start_with};
use super::World;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

/// The checkout sync execs, in order.
fn syncs(w: &World) -> Vec<ExecSpec> {
    (w.driver.execs().into_iter())
        .map(|e| e.1)
        .filter(|s| s.sync == SyncMode::Full)
        .collect()
}

/// The script the sync exec runs for the test author.
fn sync_script(name: &str, email: &str) -> String {
    format!(
        "git config --global user.name {}; echo user.name=$?; \
         git config --global user.email {}; echo user.email=$?; \
         exec git rev-parse --git-dir",
        quote(name),
        quote(email)
    )
}

#[test]
fn identity_is_set_inside_the_sync_exec_before_its_check() {
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a, b]\n", FakeDriver::new());
    assert!(v.setup.ran && v.setup.ok);
    let syncs = syncs(&w);
    assert_eq!(syncs.len(), 1);
    let sync = syncs.first().unwrap();
    let want = ["sh", "-c", &sync_script("Ann Dev", "a@x.io")].map(str::to_owned);
    assert_eq!(sync.argv, want);
    assert!(sync.output.is_some());
    assert_eq!(argvs(&w), [script_argv(&["a", "b"])]);
}

#[test]
fn without_an_author_the_sync_exec_only_checks_the_checkout() {
    let (w, _) = start_with("setup: [a]\n", FakeDriver::new());
    let syncs = syncs(&w);
    let sync = syncs.first().unwrap();
    assert_eq!(
        sync.argv,
        ["git", "rev-parse", "--git-dir"].map(str::to_owned)
    );
    assert!(sync.output.is_some());
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
    let with_identity = syncs(&w)
        .iter()
        .filter(|s| {
            s.argv
                .last()
                .is_some_and(|a| a.contains("git config --global"))
        })
        .count();
    assert_eq!(with_identity, 2);
}

#[test]
fn only_global_user_name_and_email_are_written_besides_setup_lines() {
    let (w, _) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", FakeDriver::new());
    let script = syncs(&w).remove(0).argv.remove(2);
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
fn the_sync_output_log_is_removed_after_it_is_read() {
    let driver = FakeDriver::new().exec_stdout(["user.name=0\nuser.email=0\n"]);
    let (w, _) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", driver);
    let log = w.dir.path().join("sync").join("1.log");
    assert!(log.parent().unwrap().is_dir());
    assert!(!log.exists());
    assert!(w
        .cx
        .take_warnings()
        .iter()
        .all(|x| x.code != "identity_failed"));
}
