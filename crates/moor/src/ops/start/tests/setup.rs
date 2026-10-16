use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::boxscript::setup::setup_script;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::TaskState;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::{BoxHandle, ErrorKind, StartReq, TaskId};

use super::moor_yaml::script_base_yaml;
use super::{req, world_at, World};
use crate::adapters::clock::SystemClock;
use crate::ops::common::setup::SETUP_LINE_DEADLINE;
use crate::ops::start::run;
use crate::store::Store;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::Reply;

const LINE_DEADLINE: Duration = SETUP_LINE_DEADLINE;

pub(super) fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

/// Starts `demo` in a repo whose `.moor.yaml` is `yaml`.
pub(super) fn start_with(yaml: &str, driver: FakeDriver) -> (World, moor_core::view::StartView) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let w = world_at(driver, repo.path());
    script_base_yaml(&w, Some(yaml));
    let v = run(&w.cx, &start_req()).unwrap();
    (w, v)
}

/// Starts `demo` like [`start_with`] with `git config` answering the given author.
pub(super) fn start_as(
    author: (&str, &str),
    yaml: &str,
    driver: FakeDriver,
) -> (World, moor_core::view::StartView) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let w = world_at(driver, repo.path());
    let ok = |v: &str| Reply::Ok(format!("{v}\n"));
    w.proc
        .on(Tool::Git, &["config", "--get", "user.name"], ok(author.0));
    w.proc
        .on(Tool::Git, &["config", "--get", "user.email"], ok(author.1));
    let v = run(&w.cx, &start_req()).unwrap();
    (w, v)
}

/// Whether `argv` is the exec that only syncs the checkout into the Box.
fn is_sync(argv: &[String]) -> bool {
    argv == ["git", "rev-parse", "--git-dir"]
}

/// Every exec except the checkout sync.
pub(super) fn setup_execs(w: &World) -> Vec<(BoxHandle, ExecSpec, Deadline)> {
    (w.driver.execs().into_iter())
        .filter(|e| !is_sync(&e.1.argv))
        .collect()
}

/// The argv of every exec except the checkout sync.
pub(super) fn argvs(w: &World) -> Vec<Vec<String>> {
    setup_execs(w).into_iter().map(|e| e.1.argv).collect()
}

/// Whether `argv` is the batched script that sets the Box's git identity.
pub(super) fn is_identity(argv: &[String]) -> bool {
    argv.first().map(String::as_str) == Some("sh")
        && argv
            .get(2)
            .is_some_and(|s| s.contains("git config --global"))
}

pub(super) fn is_setup_line(argv: &[String]) -> bool {
    argv.first().map(String::as_str) == Some("sh") && !is_identity(argv)
}

pub(super) fn start_req() -> StartReq {
    req("demo")
}

/// The argv that runs `steps` as the setup script.
pub(super) fn script_argv(steps: &[&str]) -> Vec<String> {
    let steps: Vec<String> = steps.iter().map(|s| (*s).to_owned()).collect();
    ["sh", "-c", &setup_script(&steps)]
        .map(str::to_owned)
        .to_vec()
}

fn kinds(w: &World, id: TaskId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

pub(super) fn warning_codes(w: &World) -> Vec<String> {
    w.cx.take_warnings().into_iter().map(|x| x.code).collect()
}

fn failed_at_set(w: &World, v: &moor_core::view::StartView) -> bool {
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    row.unwrap().unwrap().setup_failed_at.is_some()
}

#[test]
fn setup_lines_run_in_order_in_the_box_workdir() {
    let (w, v) = start_with("setup: [a, b]\n", FakeDriver::new());
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(v.setup.line, None);
    let seen: Vec<_> = setup_execs(&w)
        .into_iter()
        .map(|(h, spec, _)| {
            let cwd = spec.cwd.map(|p| p.as_str().to_owned());
            (h.as_str().to_owned(), spec.argv, cwd)
        })
        .collect();
    let want = (
        "box-1".to_owned(),
        script_argv(&["a", "b"]),
        Some("/work/cbx_1/demo".to_owned()),
    );
    assert_eq!(seen, [want]);
}

#[test]
fn setup_runs_under_one_allowance_per_line() {
    let (w, _) = start_with("setup: [a, b]\n", FakeDriver::new());
    let deadlines: Vec<Deadline> = setup_execs(&w).into_iter().map(|e| e.2).collect();
    assert_eq!(deadlines, [Deadline(LINE_DEADLINE * 2)]);
}

#[test]
fn the_checkout_syncs_in_its_own_exec_and_setup_runs_without_syncing() {
    let (w, v) = start_with("setup: [a, b]\nbox: {size: large}\n", FakeDriver::new());
    let wt =
        w.cx.store()
            .unwrap()
            .task(TaskId::new(v.task.id))
            .unwrap()
            .unwrap()
            .worktree;
    let mut execs = w.driver.execs();
    assert_eq!(execs.len(), 2, "a sync exec and one setup exec");
    let (_, setup, _) = execs.remove(1);
    let (_, sync, _) = execs.remove(0);
    assert_eq!(sync.argv, ["git", "rev-parse", "--git-dir"]);
    assert_eq!(sync.sync, SyncMode::Full);
    assert_eq!(sync.host_cwd, wt.clone().map(Into::into));
    assert_eq!(sync.class.as_deref(), Some("large"));
    assert_eq!(setup.sync, SyncMode::None);
    assert_eq!(setup.host_cwd, wt.map(Into::into));
    assert_eq!(setup.class.as_deref(), Some("large"));
}

#[test]
fn a_failing_setup_leaves_a_working_task_and_runs_nothing_more() {
    let driver = FakeDriver::new()
        .exec_results([Ok(exit(0)), Ok(exit(1))])
        .copy_text("moor setup: $ a\nmoor setup: $ b\n");
    let (w, v) = start_with("setup: [a, b, c]\n", driver);
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(v.setup.line.as_deref(), Some("b"));
    assert_eq!(setup_execs(&w).len(), 1);
    assert!(warning_codes(&w).contains(&"setup_failed".to_owned()));
    let id = TaskId::new(v.task.id);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    assert_eq!(failed.payload.get("line"), Some(&serde_json::json!("b")));
    assert!(kinds(&w, id).contains(&"working".to_owned()));
}

#[test]
fn a_failed_setup_copies_its_log_out_of_the_box_into_the_worktree() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, v) = start_with("setup: [a]\n", driver);
    let wt =
        w.cx.store()
            .unwrap()
            .task(TaskId::new(v.task.id))
            .unwrap()
            .unwrap()
            .worktree;
    let wt = std::path::PathBuf::from(wt.unwrap());
    let (_, from, to, host_cwd) = w.driver.copies().remove(0);
    assert_eq!(from.as_str(), "/work/cbx_1/demo/.moor/setup.log");
    assert_eq!(to, wt.join(".moor/setup.log"));
    assert_eq!(host_cwd, wt);
}

#[test]
fn a_failing_setup_marks_the_task_and_a_later_clean_setup_clears_it() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, v) = start_with("setup: [a]\n", driver);
    assert!(!v.setup.ok);
    assert!(failed_at_set(&w, &v));
    let id = TaskId::new(v.task.id);
    let store = w.cx.store().unwrap();
    // A re-provision reads `.moor.yaml` from the worktree, not the invocation repo.
    let wt = store.task(id).unwrap().unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [a]\n").unwrap();
    let gone = Decision::new(id).box_input(BoxInput::DriverMissing);
    store.apply(gone).unwrap();
    let again = run(&w.cx, &start_req()).unwrap();
    assert_eq!(again.task.id, v.task.id);
    assert!(again.setup.ran && again.setup.ok);
    assert!(!failed_at_set(&w, &again));
}

#[test]
fn no_setup_lines_means_setup_did_not_run() {
    let (w, v) = start_with("env: {A: a}\n", FakeDriver::new());
    assert!(!v.setup.ran);
    let execs = w.driver.execs();
    assert_eq!(execs.len(), 1);
    assert_eq!(
        execs.first().expect("one exec").1.argv,
        ["git", "rev-parse", "--git-dir"]
    );
    assert_eq!(v.task.state, TaskState::Working);
}

#[test]
fn setup_env_is_exactly_the_repo_env() {
    let yaml = "setup: [a]\nenv: {ZED: z, ALPHA: a}\n";
    let (w, _) = start_with(yaml, FakeDriver::new());
    let envs: Vec<_> = setup_execs(&w).into_iter().map(|e| e.1.env).collect();
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    assert_eq!(envs, [vec![pair("ALPHA", "a"), pair("ZED", "z")]]);
}

#[test]
fn a_timed_out_setup_counts_as_a_failing_line() {
    let driver = FakeDriver::new()
        .exec_results([Ok(exit(0)), Err(DriverError::Timeout(LINE_DEADLINE))])
        .copy_text("moor setup: $ a\nmoor setup: $ b\n");
    let (w, v) = start_with("setup: [a, b, c]\n", driver);
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(v.setup.line.as_deref(), Some("b"));
    assert_eq!(setup_execs(&w).len(), 1);
    assert!(warning_codes(&w).contains(&"setup_failed".to_owned()));
    assert!(failed_at_set(&w, &v));
}

#[test]
fn a_failing_setup_on_a_re_provisioned_working_task_records_the_failing_line() {
    let driver =
        FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(0)), Ok(exit(1))]);
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = TaskId::new(v.task.id);
    let store = w.cx.store().unwrap();
    let wt = store.task(id).unwrap().unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [a]\n").unwrap();
    script_base_yaml(&w, Some("setup: [a]\n"));
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let again = run(&w.cx, &start_req()).unwrap();
    assert_eq!(again.task.state, TaskState::Working);
    assert_eq!(again.setup.line.as_deref(), Some("a"));
    let events = store.events(id, 50).unwrap();
    let lines: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "setup_failed")
        .map(|e| e.payload.get("line").cloned())
        .collect();
    assert_eq!(lines, [Some(serde_json::json!("a"))]);
}

#[test]
fn setup_holds_the_setup_intent_and_clears_it_after() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [a]\n").unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (sink, path) = (Arc::clone(&seen), w.dir.path().join("moor.db"));
    w.driver.on_exec(move || {
        let store = Store::open(&path, Arc::new(SystemClock::real())).unwrap();
        let row = store.task(TaskId::new(1)).unwrap().unwrap();
        sink.lock().unwrap().push(row.intent);
    });
    let v = run(&w.cx, &start_req()).unwrap();
    let pid = std::process::id();
    // The checkout sync and the setup script both run under the intent.
    assert_eq!(*seen.lock().unwrap(), [Some(Intent::Setup { pid }); 2]);
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    assert_eq!(row.unwrap().unwrap().intent, None);
}

/// Starts `demo` with a failing first setup, then points its worktree at `yaml`.
fn failed_setup_then(
    yaml: &str,
    results: [Result<ExecOutcome, DriverError>; 4],
) -> (World, TaskId) {
    failed_setup_with(yaml, FakeDriver::new().exec_results(results))
}

/// Like [`failed_setup_then`] with the first setup run on `driver`.
pub(super) fn failed_setup_with(yaml: &str, driver: FakeDriver) -> (World, TaskId) {
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = TaskId::new(v.task.id);
    let wt = w.cx.store().unwrap().task(id).unwrap().unwrap().worktree;
    let wt = wt.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), yaml).unwrap();
    script_base_yaml(&w, Some(yaml));
    (w, id)
}

#[test]
fn start_after_a_failed_setup_re_runs_setup_in_the_same_box_and_clears_the_failure() {
    let (w, id) = failed_setup_then(
        "setup: [a]\n",
        [Ok(exit(0)), Ok(exit(1)), Ok(exit(0)), Ok(exit(0))],
    );
    let again = run(&w.cx, &start_req()).unwrap();
    assert_eq!(again.task.state, TaskState::Working);
    assert!(again.setup.ran && again.setup.ok);
    assert!(!failed_at_set(&w, &again));
    let scripts: Vec<_> = argvs(&w);
    assert_eq!(scripts, [script_argv(&["a"]), script_argv(&["a"])]);
    let handles: Vec<_> = setup_execs(&w).into_iter().map(|e| e.0).collect();
    assert_eq!(handles, [BoxHandle::new("box-1"), BoxHandle::new("box-1")]);
    assert_eq!(kinds(&w, id).iter().filter(|k| *k == "working").count(), 1);
}

#[test]
fn start_re_runs_setup_in_a_workdir_whose_root_holds_a_parent_component() {
    let driver = FakeDriver::new().work_root("/w/bin/../box").exec_results([
        Ok(exit(0)),
        Ok(exit(1)),
        Ok(exit(0)),
        Ok(exit(0)),
    ]);
    let (w, _) = failed_setup_with("setup: [a]\n", driver);
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    let cwds: Vec<_> = setup_execs(&w)
        .into_iter()
        .map(|e| e.1.cwd.map(|p| p.as_str().to_owned()))
        .collect();
    let stored = Some("/w/bin/../box/cbx_1/demo".to_owned());
    assert_eq!(cwds, [stored.clone(), stored]);
}

#[test]
fn start_after_a_failed_setup_that_fails_again_warns_and_stays_working() {
    let (w, id) = failed_setup_then(
        "setup: [a]\n",
        [Ok(exit(0)), Ok(exit(1)), Ok(exit(0)), Ok(exit(1))],
    );
    drop(warning_codes(&w));
    let again = run(&w.cx, &start_req()).unwrap();
    assert_eq!(again.task.state, TaskState::Working);
    assert!(again.setup.ran && !again.setup.ok);
    assert_eq!(again.setup.line.as_deref(), Some("a"));
    assert!(warning_codes(&w).contains(&"setup_failed".to_owned()));
    assert!(failed_at_set(&w, &again));
    let failed = kinds(&w, id)
        .iter()
        .filter(|k| *k == "setup_failed")
        .count();
    assert_eq!(failed, 2);
    assert_eq!(setup_execs(&w).len(), 2);
}

/// A started `demo` with a recorded Box whose setup intent belongs to `pid`.
fn interrupted_setup(pid: u32) -> (World, moor_core::view::StartView) {
    let (w, v) = start_with("setup: [a]\n", FakeDriver::new());
    let id = TaskId::new(v.task.id);
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
fn start_re_runs_setup_when_the_setup_owner_is_dead() {
    let (w, _) = interrupted_setup(4242);
    w.proc.dead(4242);
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(argvs(&w), [script_argv(&["a"]), script_argv(&["b"])]);
    let row = w.cx.store().unwrap().task(TaskId::new(again.task.id));
    assert_eq!(row.unwrap().unwrap().intent, None);
}

#[test]
fn start_leaves_setup_alone_when_the_setup_owner_is_alive() {
    let (w, _) = interrupted_setup(4242);
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(!again.setup.ran);
    assert_eq!(argvs(&w), [script_argv(&["a"])]);
    let row = w.cx.store().unwrap().task(TaskId::new(again.task.id));
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
    let id = TaskId::new(again.task.id);
    let store = w.cx.store().unwrap();
    assert!(store.live_session(id).unwrap().is_none());
    assert!(!store.has_session(id).unwrap());
}

#[test]
fn start_re_runs_setup_for_a_setting_up_task_whose_setup_owner_died() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [a]\n").unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    w.driver
        .on_exec(|| std::panic::resume_unwind(Box::new("process died mid-setup")));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, &start_req()));
    }));
    assert!(crashed.is_err());
    let id = TaskId::new(1);
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
    let again = run(&w.cx, &start_req()).unwrap();
    assert_eq!(again.task.state, TaskState::Working);
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(kinds(&w, id).iter().filter(|k| *k == "working").count(), 1);
}

#[test]
fn an_unset_author_runs_setup_without_touching_git_config() {
    let (w, v) = start_with("setup: [a, b]\n", FakeDriver::new());
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(argvs(&w), [script_argv(&["a", "b"])]);
}

#[test]
fn a_failing_identity_exec_warns_and_setup_still_runs() {
    let driver = FakeDriver::new().exec_results([Ok(exit(1))]);
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", driver);
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(argvs(&w).last(), Some(&script_argv(&["a"])));
    assert!(warning_codes(&w).contains(&"identity_failed".to_owned()));
}

#[test]
fn a_setup_run_records_its_duration_on_a_setup_finished_event() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0))]);
    let (w, v) = start_with("setup: [a]\n", driver);
    assert_eq!(v.setup.ms, Some(0));
    let id = TaskId::new(v.task.id);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let got: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "setup_finished")
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(got, [serde_json::json!({ "ms": 0, "ok": true })]);
}

#[test]
fn a_failed_setup_records_its_duration_too() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = TaskId::new(v.task.id);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let got: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "setup_finished")
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(got, [serde_json::json!({ "ms": 0, "ok": false })]);
}

/// Starts `demo` on a driver whose first execs give `results`; the engine records launches.
fn start_recording_launches(
    yaml: &str,
    results: [Result<ExecOutcome, DriverError>; 2],
) -> (
    World,
    Arc<FakeEngine>,
    Result<moor_core::view::StartView, crate::error::CommandError>,
) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let mut w = world_at(FakeDriver::new().exec_results(results), repo.path());
    let engine = Arc::new(FakeEngine::new());
    w.cx = w.cx.with_engine(engine.clone());
    let with_goal = StartReq {
        goal: Some("ship it".into()),
        ..start_req()
    };
    let got = run(&w.cx, &with_goal);
    (w, engine, got)
}

#[test]
fn a_failed_checkout_sync_aborts_the_start_without_launching_the_agent() {
    for yaml in ["env: {A: a}\n", "setup: [a, b]\n"] {
        let (w, engine, got) = start_recording_launches(yaml, [Ok(exit(1)), Ok(exit(0))]);
        let err = got.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Failed);
        let msg = err.error.to_string();
        assert!(
            msg.contains("syncing the checkout into the Box failed"),
            "{msg}"
        );
        assert!(engine.launches().is_empty());
        let id = TaskId::new(1);
        let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
        assert_eq!(row.state, TaskState::SetupFailed);
        assert!(row.intent.is_none() && row.setup_failed_at.is_some());
        assert!(kinds(&w, id).contains(&"setup_failed".to_owned()));
        let argvs: Vec<_> = w.driver.execs().into_iter().map(|e| e.1.argv).collect();
        assert_eq!(
            argvs,
            [vec![
                "git".to_owned(),
                "rev-parse".to_owned(),
                "--git-dir".to_owned()
            ]]
        );
    }
}

#[test]
fn a_failing_setup_command_after_a_good_sync_still_launches_the_agent() {
    let (w, engine, got) = start_recording_launches("setup: [a]\n", [Ok(exit(0)), Ok(exit(1))]);
    let v = got.unwrap();
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(engine.launches().len(), 1);
    let syncs: Vec<_> = w.driver.execs().into_iter().map(|e| e.1.sync).collect();
    assert_eq!(syncs, [SyncMode::Full, SyncMode::None]);
}
