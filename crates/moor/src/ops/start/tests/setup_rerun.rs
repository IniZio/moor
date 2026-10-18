//! Tests for how a start re-syncs after a failed or interrupted setup.

use std::path::Path;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::domain::task::TaskState;
use moor_core::{BoxHandle, BoxId};

use super::moor_yaml::script_base_yaml;
use super::setup::Step::{Cmd, Sync};
use super::setup::{
    argvs, execs, exit, failed_at_set, failed_setup_with, kinds, script_argv, scripted,
    setup_execs, start_req, start_with, warning_codes, Step,
};
use super::{world_at, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

/// Starts `demo` with a failing first setup, then points its worktree at `yaml`.
fn failed_setup_then(yaml: &str, results: [Step; 4]) -> (World, BoxId) {
    failed_setup_with(yaml, FakeDriver::new().exec_results(scripted(results)))
}

/// The warning `start` gives instead of re-running setup, cut to its stable ends.
fn not_rerun_warning(w: &World) -> Vec<String> {
    (w.cx.take_warnings().into_iter())
        .filter(|x| x.code == "setup_not_rerun")
        .map(|x| x.message)
        .collect()
}

const BY_HAND: &str =
    "`moor start` does not re-run it; run each `setup:` line by hand with `moor run demo -- '<line>'`";

#[test]
fn start_after_a_failed_setup_does_not_re_run_setup_and_warns() {
    let (w, id) = failed_setup_then(
        "setup: [a]\n",
        [
            Sync(Ok(exit(0))),
            Cmd(Ok(exit(1))),
            Sync(Ok(exit(0))),
            Sync(Ok(exit(0))),
        ],
    );
    drop(warning_codes(&w));
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(!again.setup.ran);
    assert_eq!(argvs(&w), [script_argv(&["a"])]);
    let [message] = not_rerun_warning(&w).try_into().unwrap();
    assert!(message.starts_with("setup failed at "));
    assert!(message.ends_with(BY_HAND), "{message}");
    assert!(failed_at_set(&w, &again));
    let failed = kinds(&w, id)
        .iter()
        .filter(|k| *k == "setup_failed")
        .count();
    assert_eq!(failed, 1);
}

#[test]
fn start_after_a_failed_setup_re_syncs_the_checkout_in_the_same_box() {
    let (w, _) = failed_setup_then(
        "setup: [a]\n",
        [
            Sync(Ok(exit(0))),
            Cmd(Ok(exit(1))),
            Sync(Ok(exit(0))),
            Sync(Ok(exit(0))),
        ],
    );
    run(&w.cx, &start_req()).unwrap();
    let handles: Vec<_> = execs(&w).into_iter().map(|e| e.0).collect();
    let box_1 = BoxHandle::new("box-1");
    assert_eq!(handles, [box_1.clone(), box_1.clone(), box_1]);
}

#[test]
fn start_after_a_failed_setup_in_a_replaced_box_runs_setup_in_the_new_box() {
    let (w, id) = failed_setup_then(
        "setup: [a]\n",
        [
            Sync(Ok(exit(0))),
            Cmd(Ok(exit(1))),
            Sync(Ok(exit(0))),
            Cmd(Ok(exit(0))),
        ],
    );
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    drop(warning_codes(&w));
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(argvs(&w), [script_argv(&["a"]), script_argv(&["a"])]);
    let handles: Vec<_> = setup_execs(&w).into_iter().map(|e| e.0).collect();
    assert_eq!(handles, [BoxHandle::new("box-1"), BoxHandle::new("box-2")]);
    assert!(not_rerun_warning(&w).is_empty());
    assert!(!failed_at_set(&w, &again));
}

/// A started `demo` with a recorded Box whose setup intent belongs to `pid`.
fn interrupted_setup(pid: u32) -> (World, moor_core::view::StartView) {
    let (w, v) = start_with("setup: [a]\n", FakeDriver::new());
    let id = BoxId::new(v.task.id);
    let wt = w.cx.store().unwrap().task(id).unwrap().unwrap().worktree;
    let wt = wt.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [b]\n").unwrap();
    script_base_yaml(&w, Some("setup: [b]\n"));
    let held = Decision::new(id).intent(IntentChange::Set(Intent::Setup { pid }));
    w.cx.store().unwrap().apply(held).unwrap();
    (w, v)
}

#[test]
fn start_does_not_re_run_setup_when_the_setup_owner_is_dead() {
    let (w, _) = interrupted_setup(4242);
    w.proc.dead(4242);
    drop(warning_codes(&w));
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(!again.setup.ran);
    assert_eq!(argvs(&w), [script_argv(&["a"])]);
    let [message] = not_rerun_warning(&w).try_into().unwrap();
    assert!(message.starts_with("setup was interrupted; "));
    assert!(message.ends_with(BY_HAND), "{message}");
    assert!(failed_at_set(&w, &again));
    let row = w.cx.store().unwrap().task(BoxId::new(again.task.id));
    assert_eq!(row.unwrap().unwrap().intent, None);
}

#[test]
fn start_after_an_interrupted_setup_re_syncs_the_checkout_in_the_same_box() {
    let (w, _) = interrupted_setup(4242);
    w.proc.dead(4242);
    drop(warning_codes(&w));
    let syncs = |w: &World| {
        let all = execs(w).into_iter();
        let all = all.filter(|e| e.1.argv == ["git", "rev-parse", "--git-dir"]);
        all.map(|e| e.0).collect::<Vec<_>>()
    };
    let before = syncs(&w).len();
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(!again.setup.ran);
    assert_eq!(argvs(&w), [script_argv(&["a"])]);
    let after = syncs(&w);
    assert_eq!(after.len(), before + 1);
    assert_eq!(after.last(), Some(&BoxHandle::new("box-1")));
    let [message] = not_rerun_warning(&w).try_into().unwrap();
    assert!(message.starts_with("setup was interrupted; "));
}

#[test]
fn start_leaves_setup_alone_when_the_setup_owner_is_alive() {
    let (w, _) = interrupted_setup(4242);
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(!again.setup.ran);
    assert_eq!(argvs(&w), [script_argv(&["a"])]);
    let row = w.cx.store().unwrap().task(BoxId::new(again.task.id));
    assert_eq!(
        row.unwrap().unwrap().intent,
        Some(Intent::Setup { pid: 4242 })
    );
}

#[test]
fn start_on_a_setting_up_task_with_a_live_setup_owner_records_no_session() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [a]\n").unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    w.driver
        .on_exec(|| std::panic::resume_unwind(Box::new("process died mid-setup")));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, &start_req()));
    }));
    assert!(crashed.is_err());
    w.driver.on_exec(|| {});
    let again = run(&w.cx, &start_req()).unwrap();
    assert_eq!(again.task.state, TaskState::SettingUp);
    let id = BoxId::new(again.task.id);
    let store = w.cx.store().unwrap();
    assert!(store.live_session(id).unwrap().is_none());
    assert!(!store.has_session(id).unwrap());
}

#[test]
fn start_does_not_re_run_setup_for_a_setting_up_task_whose_setup_owner_died() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [a]\n").unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    w.driver
        .on_exec(|| std::panic::resume_unwind(Box::new("process died mid-setup")));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, &start_req()));
    }));
    assert!(crashed.is_err());
    let id = BoxId::new(1);
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    assert_eq!(row.state, TaskState::SettingUp);
    assert_eq!(row.boxed.map(|b| b.state), Some(BoxState::Ready));
    assert_eq!(
        row.intent,
        Some(Intent::Setup {
            pid: std::process::id()
        })
    );
    w.driver.on_exec(|| {});
    w.proc.dead(std::process::id());
    drop(warning_codes(&w));
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(!again.setup.ran);
    assert_eq!(not_rerun_warning(&w).len(), 1);
    assert!(failed_at_set(&w, &again));
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    assert_eq!(row.intent, None);
    assert_eq!(row.state, TaskState::Working);
}
