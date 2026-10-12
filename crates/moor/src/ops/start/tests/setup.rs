use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Intent};
use moor_core::domain::task::TaskState;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::{StartReq, TaskId};

use super::{req, world_at, World};
use crate::adapters::clock::SystemClock;
use crate::ops::common::setup::SETUP_LINE_DEADLINE;
use crate::ops::start::run;
use crate::store::Store;
use crate::testing::driver::FakeDriver;

const LINE_DEADLINE: Duration = SETUP_LINE_DEADLINE;

fn exit(code: i32) -> ExecOutcome {
    ExecOutcome { code }
}

/// Starts `demo` in a repo whose `.moor.yaml` is `yaml`.
fn start_with(yaml: &str, driver: FakeDriver) -> (World, moor_core::view::StartView) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let w = world_at(driver, repo.path());
    let v = run(&w.cx, &start_req()).unwrap();
    (w, v)
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
