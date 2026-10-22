//! Tests for in-flight run detection, ghost-run healing and recorded-run lookups.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::domain::decision::{Decision, Intent, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName, RunId, Sha};

use crate::testing::clock::FakeClock;

use super::{NewRun, RunOutcome};
use crate::store::{FollowClaim, Store};

const SHA_A: &str = "abcdef1234567890abcdef1234567890abcdef12";
const SHA_B: &str = "abcd00001234567890abcdef1234567890abcdef";

/// Open a store in a temp dir.
fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Insert a box by name.
fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    store.apply(Decision::insert(nt)).unwrap().box_id
}

/// Begin a run on `box_id` owned by `pid`, holding its follow claim.
fn follow(
    store: &Store,
    box_id: BoxId,
    record: Option<&str>,
    sha: Option<&str>,
    pid: u32,
) -> (RunId, FollowClaim) {
    store
        .begin_run(NewRun {
            box_id,
            argv: vec!["cargo".into(), "test".into()],
            record: record.map(str::to_owned),
            sha: sha.map(|s| Sha::parse(s).unwrap()),
            pid,
        })
        .unwrap()
}

/// Begin a run on `box_id` owned by `pid` whose follower is already gone.
fn begin(store: &Store, box_id: BoxId, record: Option<&str>, sha: Option<&str>, pid: u32) -> RunId {
    follow(store, box_id, record, sha, pid).0
}

/// Finish a run with an exit code.
fn finish(store: &Store, run: RunId, exit: i32) {
    let out = RunOutcome {
        exit: Some(exit),
        interrupted: false,
        error: None,
    };
    store.finish_run(run, out).unwrap();
}

/// How many `ran` events a box has.
fn ran_events(store: &Store, box_id: BoxId) -> usize {
    let events = store.events(box_id, 100).unwrap();
    events.iter().filter(|e| e.kind == "ran").count()
}

#[test]
fn in_flight_run_is_returned_while_its_claim_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let box_id = add(&store, "t");
    assert!(store.in_flight_run(box_id).unwrap().is_none());

    let (run, claim) = follow(&store, box_id, None, None, 42);
    let seen = store.in_flight_run(box_id).unwrap().unwrap();
    assert_eq!((seen.id, seen.box_id, seen.pid), (run, box_id, 42));
    assert_eq!(seen.finished_at, None);
    assert_eq!(ran_events(&store, box_id), 0);

    finish(&store, run, 0);
    drop(claim);
    assert!(store.in_flight_run(box_id).unwrap().is_none());
    assert_eq!(ran_events(&store, box_id), 1);
}

#[test]
fn a_live_pid_without_a_claim_is_not_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let box_id = add(&store, "t");
    let run = begin(&store, box_id, None, None, std::process::id());

    assert!(store.in_flight_run(box_id).unwrap().is_none());
    let row = store.last_run(box_id).unwrap().unwrap();
    assert_eq!(row.id, run);
    assert!(row.finished_at.is_some() && row.interrupted);
}

/// Names the state dir and Box a child process follows a new run of, until it is killed.
const HOLDER: &str = "MOOR_TEST_FOLLOW_HOLDER";

#[test]
fn follow_until_killed() {
    let Some(spec) = std::env::var_os(HOLDER) else {
        return;
    };
    let spec = spec.into_string().unwrap();
    let (dir, box_id) = spec.rsplit_once(':').unwrap();
    let store = open(Path::new(dir));
    let box_id = BoxId::new(box_id.parse().unwrap());
    let _claim = follow(&store, box_id, None, None, std::process::id());
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn a_killed_follower_leaves_its_run_interrupted() {
    if std::env::var_os(HOLDER).is_some() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let box_id = add(&store, "t");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "store::runs::tests::follow_until_killed"])
        .env(HOLDER, format!("{}:{}", dir.path().display(), box_id.get()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let begun = (0..1000).find_map(|_| {
        thread::sleep(Duration::from_millis(10));
        store.last_run(box_id).unwrap()
    });
    if begun.is_none() {
        child.kill().ok();
    }
    let begun = begun.expect("the child began its run");
    let seen = store.in_flight_run(box_id).unwrap().map(|r| r.id);
    assert_eq!(seen, Some(begun.id), "a held claim is in flight");

    let pid = rustix::process::Pid::from_child(&child);
    rustix::process::kill_process(pid, rustix::process::Signal::KILL).unwrap();
    child.wait().unwrap();

    assert!(store.in_flight_run(box_id).unwrap().is_none());
    let row = store.last_run(box_id).unwrap().unwrap();
    assert_eq!(row.id, begun.id);
    assert!(row.finished_at.is_some() && row.interrupted, "{row:?}");
    assert_eq!(ran_events(&store, box_id), 1);
}

#[test]
fn unclaimed_run_is_interrupted_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let box_id = add(&store, "t");
    let run = begin(&store, box_id, Some("lint"), Some(SHA_A), 42);

    assert!(store.in_flight_run(box_id).unwrap().is_none());
    assert_eq!(ran_events(&store, box_id), 1);
    assert_eq!(store.box_row(box_id).unwrap().unwrap().intent, None);
    let events = store.events(box_id, 100).unwrap();
    assert_eq!(
        events.last().unwrap().payload,
        serde_json::json!({"run": run.get(), "exit": null, "interrupted": true})
    );

    assert!(store.in_flight_run(box_id).unwrap().is_none());
    assert_eq!(ran_events(&store, box_id), 1);

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
    let box_id = add(&store, "t");
    let run = begin(&store, box_id, Some("lint"), Some(SHA_A), 42);
    finish(&store, run, 3);

    finish(&store, run, 9);
    assert!(store.in_flight_run(box_id).unwrap().is_none());
    assert_eq!(ran_events(&store, box_id), 1);
    let rows = store.recorded_runs("abcdef").unwrap();
    let row = rows.first().unwrap();
    assert_eq!((row.exit, row.interrupted), (Some(3), false));

    let unknown = store.finish_run(
        RunId::new(999),
        RunOutcome {
            exit: None,
            interrupted: true,
            error: None,
        },
    );
    assert!(unknown.is_err());
}

#[test]
fn healing_keeps_a_running_intent_of_another_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let box_id = add(&store, "t");
    let old = begin(&store, box_id, None, None, 1);
    let (new, _claim) = follow(&store, box_id, None, None, 2);

    let seen = store.in_flight_run(box_id).unwrap().unwrap();
    assert_eq!(seen.id, new);
    assert_eq!(ran_events(&store, box_id), 1);
    assert_eq!(
        store.box_row(box_id).unwrap().unwrap().intent,
        Some(Intent::Running { run: new, pid: 2 })
    );
    assert_ne!(old, new);
}

#[test]
fn recorded_runs_match_prefix_across_boxes_and_names() {
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
    assert_eq!(rows.get(1).unwrap().box_id, b);
    assert_eq!(rows.get(2).unwrap().record.as_deref(), Some("test"));
    assert_eq!(store.recorded_runs("abcd").unwrap().len(), 4);
    assert!(store.recorded_runs("ffff").unwrap().is_empty());
}

#[test]
fn recorded_runs_carry_what_the_gate_needs() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let box_id = add(&store, "t");
    let run = begin(&store, box_id, Some("lint"), Some(SHA_A), 2);
    finish(&store, run, 1);

    let rows = store.recorded_runs("abcdef").unwrap();
    let last = rows.first().unwrap();
    assert_eq!((last.id, last.box_id), (run, box_id));
    assert_eq!(last.sha.as_ref().unwrap().as_str(), SHA_A);
    assert_eq!(last.record.as_deref(), Some("lint"));
    assert_eq!(last.argv, ["cargo", "test"]);
    assert_eq!((last.exit, last.interrupted), (Some(1), false));
    assert_eq!(last.started_at, Ts::from_millis(1_000));
    assert_eq!(last.finished_at, Some(Ts::from_millis(1_000)));
}

#[test]
fn refused_run_emits_one_ran_event_without_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let box_id = add(&store, "t");
    let before = store.box_row(box_id).unwrap().unwrap();
    store.refused_run(box_id).unwrap();
    let events = store.events(box_id, 100).unwrap();
    let ran: Vec<_> = events.iter().filter(|e| e.kind == "ran").collect();
    assert_eq!(ran.len(), 1);
    let payload = &ran.first().unwrap().payload;
    assert!(payload["run"].is_null(), "{payload}");
    assert!(payload["exit"].is_null(), "{payload}");
    assert_eq!(payload["interrupted"], false);
    assert!(store.last_run(box_id).unwrap().is_none());
    assert_eq!(
        store.box_row(box_id).unwrap().unwrap().intent,
        before.intent
    );
}

#[test]
fn refused_run_leaves_an_in_flight_run_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let box_id = add(&store, "t");
    let (run, _claim) = follow(&store, box_id, None, None, 7);
    store.refused_run(box_id).unwrap();
    let live = store.in_flight_run(box_id).unwrap().unwrap();
    assert_eq!(live.id, run);
    assert_eq!(live.finished_at, None);
    assert_eq!(
        store.box_row(box_id).unwrap().unwrap().intent,
        Some(Intent::Running { run, pid: 7 })
    );
    assert_eq!(ran_events(&store, box_id), 1);
}
