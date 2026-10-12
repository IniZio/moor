//! Tests for in-flight run detection, ghost-run healing and recorded-run lookups.

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, Intent, NewTask};
use moor_core::seam::clock::Ts;
use moor_core::{RunId, Sha, TaskId, TaskName};

use crate::testing::clock::FakeClock;

use super::{NewRun, RunOutcome};
use crate::store::Store;

const SHA_A: &str = "abcdef1234567890abcdef1234567890abcdef12";
const SHA_B: &str = "abcd00001234567890abcdef1234567890abcdef";

/// Open a store in a temp dir.
fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Insert a task by name.
fn add(store: &Store, name: &str) -> TaskId {
    let nt = NewTask {
        name: TaskName::parse(name).unwrap(),
        repo: "r".into(),
    };
    store.apply(Decision::insert(nt)).unwrap().task
}

/// Begin a run on `task` owned by `pid`.
fn begin(store: &Store, task: TaskId, record: Option<&str>, sha: Option<&str>, pid: u32) -> RunId {
    store
        .begin_run(NewRun {
            task,
            argv: vec!["cargo".into(), "test".into()],
            record: record.map(str::to_owned),
            sha: sha.map(|s| Sha::parse(s).unwrap()),
            sync: "tree".into(),
            pid,
        })
        .unwrap()
}

/// Finish a run with an exit code.
fn finish(store: &Store, run: RunId, exit: i32) {
    let out = RunOutcome {
        exit: Some(exit),
        interrupted: false,
    };
    store.finish_run(run, out).unwrap();
}

/// How many `ran` events a task has.
fn ran_events(store: &Store, task: TaskId) -> usize {
    let events = store.events(task, 100).unwrap();
    events.iter().filter(|e| e.kind == "ran").count()
}

#[test]
fn in_flight_run_is_returned_while_pid_is_alive() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let task = add(&store, "t");
    assert!(store.in_flight_run(task, &|_| true).unwrap().is_none());

    let run = begin(&store, task, None, None, 42);
    let seen = store
        .in_flight_run(task, &|pid| pid == 42)
        .unwrap()
        .unwrap();
    assert_eq!((seen.id, seen.task, seen.pid), (run, task, 42));
    assert_eq!(seen.finished_at, None);
    assert_eq!(ran_events(&store, task), 0);

    finish(&store, run, 0);
    assert!(store.in_flight_run(task, &|_| true).unwrap().is_none());
}

#[test]
fn dead_pid_run_is_interrupted_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let task = add(&store, "t");
    let run = begin(&store, task, Some("lint"), Some(SHA_A), 42);

    assert!(store.in_flight_run(task, &|_| false).unwrap().is_none());
    assert_eq!(ran_events(&store, task), 1);
    assert_eq!(store.task(task).unwrap().unwrap().intent, None);
    let events = store.events(task, 100).unwrap();
    assert_eq!(
        events.last().unwrap().payload,
        serde_json::json!({"run": run.get(), "exit": null, "interrupted": true})
    );

    assert!(store.in_flight_run(task, &|_| false).unwrap().is_none());
    assert_eq!(ran_events(&store, task), 1);

    let rows = store.recorded_runs("abcdef").unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows.first().unwrap();
    assert_eq!((row.exit, row.interrupted), (None, true));
    assert!(row.finished_at.is_some());
}

#[test]
fn finishing_a_finished_run_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let task = add(&store, "t");
    let run = begin(&store, task, Some("lint"), Some(SHA_A), 42);
    finish(&store, run, 3);

    finish(&store, run, 9);
    assert!(store.in_flight_run(task, &|_| false).unwrap().is_none());
    assert_eq!(ran_events(&store, task), 1);
    let rows = store.recorded_runs("abcdef").unwrap();
    let row = rows.first().unwrap();
    assert_eq!((row.exit, row.interrupted), (Some(3), false));

    let unknown = store.finish_run(
        RunId::new(999),
        RunOutcome {
            exit: None,
            interrupted: true,
        },
    );
    assert!(unknown.is_err());
}

#[test]
fn healing_keeps_a_running_intent_of_another_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let task = add(&store, "t");
    let old = begin(&store, task, None, None, 1);
    let new = begin(&store, task, None, None, 2);

    let seen = store.in_flight_run(task, &|pid| pid == 2).unwrap().unwrap();
    assert_eq!(seen.id, new);
    assert_eq!(ran_events(&store, task), 1);
    assert_eq!(
        store.task(task).unwrap().unwrap().intent,
        Some(Intent::Running { run: new, pid: 2 })
    );
    assert_ne!(old, new);
}

#[test]
fn recorded_runs_match_prefix_across_tasks_and_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let a = add(&store, "a");
    let b = add(&store, "b");
    let first = begin(&store, a, Some("lint"), Some(SHA_A), 1);
    finish(&store, first, 0);
    let second = begin(&store, b, Some("lint"), Some(SHA_A), 2);
    finish(&store, second, 1);
    let other_name = begin(&store, a, Some("test"), Some(SHA_A), 3);
    finish(&store, other_name, 0);
    let other_sha = begin(&store, a, Some("lint"), Some(SHA_B), 4);
    finish(&store, other_sha, 0);
    let unrecorded = begin(&store, a, None, Some(SHA_A), 5);
    finish(&store, unrecorded, 0);
    let _open = begin(&store, a, Some("lint"), Some(SHA_A), 6);

    let rows = store.recorded_runs("ABCDEF12").unwrap();
    let ids: Vec<_> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, [first, second, other_name]);
    assert_eq!(rows.get(1).unwrap().task, b);
    assert_eq!(rows.get(2).unwrap().record.as_deref(), Some("test"));
    assert_eq!(store.recorded_runs("abcd").unwrap().len(), 4);
    assert!(store.recorded_runs("ffff").unwrap().is_empty());
}

#[test]
fn recorded_runs_carry_what_the_gate_needs() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let task = add(&store, "t");
    let run = begin(&store, task, Some("lint"), Some(SHA_A), 2);
    finish(&store, run, 1);

    let rows = store.recorded_runs("abcdef").unwrap();
    let last = rows.first().unwrap();
    assert_eq!((last.id, last.task), (run, task));
    assert_eq!(last.sha.as_ref().unwrap().as_str(), SHA_A);
    assert_eq!(last.record.as_deref(), Some("lint"));
    assert_eq!(last.argv, ["cargo", "test"]);
    assert_eq!((last.exit, last.interrupted), (Some(1), false));
    assert_eq!(last.started_at, Ts::from_millis(1_000));
    assert_eq!(last.finished_at, Some(Ts::from_millis(1_000)));
}
