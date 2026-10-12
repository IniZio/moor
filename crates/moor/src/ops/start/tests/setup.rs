use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::domain::task::TaskState;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::{BoxHandle, StartReq, TaskId};

use super::{req, world_at, World};
use crate::adapters::clock::SystemClock;
use crate::ops::common::setup::SETUP_LINE_DEADLINE;
use crate::ops::start::run;
use crate::store::Store;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

const LINE_DEADLINE: Duration = SETUP_LINE_DEADLINE;

fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

/// Starts `demo` in a repo whose `.moor.yaml` is `yaml`.
fn start_with(yaml: &str, driver: FakeDriver) -> (World, moor_core::view::StartView) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let w = world_at(driver, repo.path());
    let v = run(&w.cx, &start_req()).unwrap();
    (w, v)
}

/// Starts `demo` like [`start_with`] with `git config` answering the given author.
fn start_as(
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

fn argvs(w: &World) -> Vec<Vec<String>> {
    w.driver.execs().into_iter().map(|e| e.1.argv).collect()
}

fn is_setup_line(argv: &[String]) -> bool {
    argv.first().map(String::as_str) == Some("sh")
}

fn git_config(key: &str, value: &str) -> Vec<String> {
    ["git", "config", "--global", key, value]
        .map(str::to_owned)
        .to_vec()
}

fn start_req() -> StartReq {
    req("demo")
}

fn lines(w: &World) -> Vec<String> {
    w.driver
        .execs()
        .iter()
        .filter_map(|(_, spec, _)| spec.argv.last().cloned())
        .collect()
}

fn kinds(w: &World, id: TaskId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn warning_codes(w: &World) -> Vec<String> {
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
    let seen: Vec<_> = w
        .driver
        .execs()
        .into_iter()
        .map(|(h, spec, _)| {
            let cwd = spec.cwd.map(|p| p.as_str().to_owned());
            (h.as_str().to_owned(), spec.argv, cwd)
        })
        .collect();
    let want = |line: &str| {
        (
            "box-1".to_owned(),
            vec!["sh".to_owned(), "-c".to_owned(), line.to_owned()],
            Some("/work/box-1".to_owned()),
        )
    };
    assert_eq!(seen, [want("a"), want("b")]);
}

#[test]
fn setup_lines_run_under_the_line_deadline() {
    let (w, _) = start_with("setup: [a, b]\n", FakeDriver::new());
    let deadlines: Vec<Deadline> = w.driver.execs().into_iter().map(|e| e.2).collect();
    assert_eq!(
        deadlines,
        [Deadline(LINE_DEADLINE), Deadline(LINE_DEADLINE)]
    );
}

#[test]
fn a_failing_setup_line_leaves_a_working_task_and_stops_later_lines() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, v) = start_with("setup: [a, b, c]\n", driver);
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(v.setup.line.as_deref(), Some("b"));
    assert_eq!(lines(&w), ["a", "b"]);
    assert!(warning_codes(&w).contains(&"setup_failed".to_owned()));
    let id = TaskId::new(v.task.id);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    assert_eq!(failed.payload.get("line"), Some(&serde_json::json!("b")));
    assert!(kinds(&w, id).contains(&"working".to_owned()));
}

#[test]
fn a_failing_setup_marks_the_task_and_a_later_clean_setup_clears_it() {
    let driver = FakeDriver::new().exec_results([Ok(exit(1))]);
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
    let (w, v) = start_with("egress: [example.com]\n", FakeDriver::new());
    assert!(!v.setup.ran);
    assert!(w.driver.execs().is_empty());
    assert_eq!(v.task.state, TaskState::Working);
}

#[test]
fn setup_env_is_exactly_the_repo_env() {
    let yaml = "setup: [a]\nenv: {ZED: z, ALPHA: a}\negress: [example.com]\n";
    let (w, _) = start_with(yaml, FakeDriver::new());
    let envs: Vec<_> = w.driver.execs().into_iter().map(|e| e.1.env).collect();
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    assert_eq!(envs, [vec![pair("ALPHA", "a"), pair("ZED", "z")]]);
}

#[test]
fn a_timed_out_setup_line_counts_as_a_failing_line() {
    let driver =
        FakeDriver::new().exec_results([Ok(exit(0)), Err(DriverError::Timeout(LINE_DEADLINE))]);
    let (w, v) = start_with("setup: [a, b, c]\n", driver);
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(v.setup.line.as_deref(), Some("b"));
    assert_eq!(lines(&w), ["a", "b"]);
    assert!(warning_codes(&w).contains(&"setup_failed".to_owned()));
    assert!(failed_at_set(&w, &v));
}

#[test]
fn a_failing_setup_on_a_re_provisioned_working_task_records_the_failing_line() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = TaskId::new(v.task.id);
    let store = w.cx.store().unwrap();
    let wt = store.task(id).unwrap().unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [a]\n").unwrap();
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
    assert_eq!(*seen.lock().unwrap(), [Some(Intent::Setup { pid })]);
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    assert_eq!(row.unwrap().unwrap().intent, None);
}

/// Starts `demo` with a failing first setup, then points its worktree at `yaml`.
fn failed_setup_then(
    yaml: &str,
    results: [Result<ExecOutcome, DriverError>; 2],
) -> (World, TaskId) {
    failed_setup_with(yaml, FakeDriver::new().exec_results(results))
}

/// Like [`failed_setup_then`] with the first setup run on `driver`.
fn failed_setup_with(yaml: &str, driver: FakeDriver) -> (World, TaskId) {
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = TaskId::new(v.task.id);
    let wt = w.cx.store().unwrap().task(id).unwrap().unwrap().worktree;
    let wt = wt.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), yaml).unwrap();
    (w, id)
}

#[test]
fn start_after_a_failed_setup_re_runs_setup_in_the_same_box_and_clears_the_failure() {
    let (w, id) = failed_setup_then("setup: [a]\n", [Ok(exit(1)), Ok(exit(0))]);
    let again = run(&w.cx, &start_req()).unwrap();
    assert_eq!(again.task.state, TaskState::Working);
    assert!(again.setup.ran && again.setup.ok);
    assert!(!failed_at_set(&w, &again));
    assert_eq!(lines(&w), ["a", "a"]);
    let handles: Vec<_> = w.driver.execs().into_iter().map(|e| e.0).collect();
    assert_eq!(handles, [BoxHandle::new("box-1"), BoxHandle::new("box-1")]);
    assert_eq!(kinds(&w, id).iter().filter(|k| *k == "working").count(), 1);
}

#[test]
fn start_re_runs_setup_in_a_workdir_whose_root_holds_a_parent_component() {
    let driver = FakeDriver::new()
        .work_root("/w/bin/../box")
        .exec_results([Ok(exit(1)), Ok(exit(0))]);
    let (w, _) = failed_setup_with("setup: [a]\n", driver);
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    let cwds: Vec<_> = w
        .driver
        .execs()
        .into_iter()
        .map(|e| e.1.cwd.map(|p| p.as_str().to_owned()))
        .collect();
    let stored = Some("/w/bin/../box/box-1".to_owned());
    assert_eq!(cwds, [stored.clone(), stored]);
}

#[test]
fn start_after_a_failed_setup_that_fails_again_warns_and_stays_working() {
    let (w, id) = failed_setup_then("setup: [a]\n", [Ok(exit(1)), Ok(exit(1))]);
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
    assert_eq!(w.driver.execs().len(), 2);
}

/// A started `demo` with a recorded Box whose setup intent belongs to `pid`.
fn interrupted_setup(pid: u32) -> (World, moor_core::view::StartView) {
    let (w, v) = start_with("setup: [a]\n", FakeDriver::new());
    let id = TaskId::new(v.task.id);
    let wt = w.cx.store().unwrap().task(id).unwrap().unwrap().worktree;
    let wt = wt.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [b]\n").unwrap();
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
    assert_eq!(lines(&w), ["a", "b"]);
    let row = w.cx.store().unwrap().task(TaskId::new(again.task.id));
    assert_eq!(row.unwrap().unwrap().intent, None);
}

#[test]
fn start_leaves_setup_alone_when_the_setup_owner_is_alive() {
    let (w, _) = interrupted_setup(4242);
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(!again.setup.ran);
    assert_eq!(lines(&w), ["a"]);
    let row = w.cx.store().unwrap().task(TaskId::new(again.task.id));
    assert_eq!(
        row.unwrap().unwrap().intent,
        Some(Intent::Setup { pid: 4242 })
    );
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
fn identity_is_set_in_the_box_before_the_first_setup_line() {
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a, b]\n", FakeDriver::new());
    assert!(v.setup.ran && v.setup.ok);
    let sh = |l: &str| ["sh", "-c", l].map(str::to_owned).to_vec();
    assert_eq!(
        argvs(&w),
        [
            git_config("user.name", "Ann Dev"),
            git_config("user.email", "a@x.io"),
            sh("a"),
            sh("b"),
        ]
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
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    run(&w.cx, &start_req()).unwrap();
    let configs = argvs(&w).iter().filter(|a| !is_setup_line(a)).count();
    assert_eq!(configs, 4);
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
    let before = argvs(&w).len();
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    let sh = |l: &str| ["sh", "-c", l].map(str::to_owned).to_vec();
    assert_eq!(
        argvs_after(&w, before),
        [
            git_config("user.name", "Ann Dev"),
            git_config("user.email", "a@x.io"),
            sh("b"),
        ]
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
    let held = Decision::new(id).intent(IntentChange::Set(Intent::Setup { pid: 4242 }));
    w.cx.store().unwrap().apply(held).unwrap();
    w.proc.dead(4242);
    let before = argvs(&w).len();
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    let sh = |l: &str| ["sh", "-c", l].map(str::to_owned).to_vec();
    assert_eq!(
        argvs_after(&w, before),
        [
            git_config("user.name", "Ann Dev"),
            git_config("user.email", "a@x.io"),
            sh("b"),
        ]
    );
}

#[test]
fn an_unset_author_runs_setup_without_touching_git_config() {
    let (w, v) = start_with("setup: [a, b]\n", FakeDriver::new());
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(lines(&w), ["a", "b"]);
    assert!(argvs(&w).iter().all(|a| is_setup_line(a)));
}

#[test]
fn only_global_user_name_and_email_are_written_besides_setup_lines() {
    let (w, _) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", FakeDriver::new());
    let others: Vec<_> = argvs(&w)
        .into_iter()
        .filter(|a| !is_setup_line(a))
        .collect();
    assert_eq!(
        others,
        [
            git_config("user.name", "Ann Dev"),
            git_config("user.email", "a@x.io")
        ]
    );
    let joined = argvs(&w).concat().join(" ");
    assert!(!joined.contains(".gitconfig") && !joined.contains("hosts.yml"));
}

#[test]
fn a_failing_identity_exec_warns_and_setup_still_runs() {
    let driver = FakeDriver::new().exec_results([Ok(exit(1))]);
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", driver);
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(lines(&w).last().map(String::as_str), Some("a"));
    assert!(warning_codes(&w).contains(&"identity_failed".to_owned()));
}
