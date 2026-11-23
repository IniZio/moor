//! Workflows and the record of their firings.

use std::time::Duration;

use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Clock as _;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::workflow::{
    NewRun, Plan, RunCause, RunEnd, RunOutcome, RunState, StepId, TimeoutMinutes, WorkflowDef,
    WorkflowId, WorkflowName, WorkflowRun, WorkflowSchedule, WorkflowState,
};
use moor_core::BoxId;

use crate::store::tests::open;
use crate::store::Store;
use crate::testing::runs::{refused, running, skipped, timed_out};

pub(super) fn repo(s: &str) -> ForgeRepo {
    ForgeRepo::parse(s).unwrap()
}

pub(super) fn name(s: &str) -> WorkflowName {
    WorkflowName::parse(s).unwrap()
}

pub(super) fn def(n: &str, repos: &[&str], enabled: bool) -> WorkflowDef {
    WorkflowDef::new(
        name(n),
        Some(WorkflowSchedule::parse("0 9 * * *", "Europe/London").unwrap()),
        repos.iter().map(|r| repo(r)).collect(),
        "fix the security alerts",
        (enabled, TimeoutMinutes::DEFAULT),
    )
    .unwrap()
}

pub(super) fn user(store: &Store, identity: &str) -> UserId {
    store
        .enroll(&Identity::parse(identity).unwrap(), None)
        .unwrap()
        .0
}

pub(super) fn run(workflow: WorkflowId, r: &str, due: i64, state: RunState) -> NewRun {
    NewRun {
        workflow,
        repo: repo(r),
        due_at: Ts::from_millis(due),
        state,
        recorded_at: Ts::from_millis(due + 5),
        plan: None,
        cause: RunCause::Cron,
    }
}

/// `run` as it was recorded, without the id the store gave it.
fn recorded(run: &WorkflowRun) -> NewRun {
    NewRun {
        workflow: run.workflow,
        repo: run.repo.clone(),
        due_at: run.due_at,
        state: run.state.clone(),
        recorded_at: run.recorded_at,
        plan: None,
        cause: run.cause.clone(),
    }
}

fn recorded_all(runs: &[WorkflowRun]) -> Vec<NewRun> {
    runs.iter().map(recorded).collect()
}

#[test]
fn a_workflow_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one", "a/two"], true))
        .unwrap();
    let got = store.workflow(alice, &name("daily")).unwrap().unwrap();
    assert_eq!(got.id, id);
    assert_eq!(got.owner, alice);
    assert_eq!(got.repos, [repo("a/one"), repo("a/two")]);
    assert_eq!(got.prompt, "fix the security alerts");
    assert_eq!(got.schedule.as_ref().unwrap().expr(), "0 9 * * *");
    assert_eq!(got.schedule.as_ref().unwrap().zone(), "Europe/London");
    assert_eq!(
        got.state,
        WorkflowState::Enabled {
            since: got.created_at
        }
    );
    assert_eq!(got.timeout, TimeoutMinutes::DEFAULT);
    assert!(store.workflow(alice, &name("absent")).unwrap().is_none());
}

#[test]
fn a_workflow_with_no_cron_round_trips_and_a_cron_can_be_added_and_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let mut by_hand = def("manual", &["a/one"], true);
    by_hand.schedule = None;
    store.put_workflow(alice, &by_hand).unwrap();
    let got = store.workflow(alice, &name("manual")).unwrap().unwrap();
    assert_eq!(got.schedule, None);
    store
        .put_workflow(alice, &def("manual", &["a/one"], true))
        .unwrap();
    let got = store.workflow(alice, &name("manual")).unwrap().unwrap();
    assert_eq!(got.schedule.as_ref().unwrap().expr(), "0 9 * * *");
    store.put_workflow(alice, &by_hand).unwrap();
    let got = store.workflow(alice, &name("manual")).unwrap().unwrap();
    assert_eq!(got.schedule, None);
    assert_eq!(store.workflows(alice).unwrap().len(), 1);
}

#[test]
fn workflows_are_listed_by_name_per_owner() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (alice, bob) = (user(&store, "local:1"), user(&store, "local:2"));
    for (owner, n) in [(alice, "weekly"), (alice, "daily"), (bob, "daily")] {
        store
            .put_workflow(owner, &def(n, &["a/one"], true))
            .unwrap();
    }
    let names: Vec<_> = store
        .workflows(alice)
        .unwrap()
        .iter()
        .map(|w| w.name.to_string())
        .collect();
    assert_eq!(names, ["daily", "weekly"]);
    assert_eq!(store.workflows(bob).unwrap().len(), 1);
}

#[test]
fn putting_a_name_again_replaces_the_definition_and_keeps_its_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    let first = store.workflow(alice, &name("daily")).unwrap().unwrap();
    store
        .record_workflow_run(&run(id, "a/one", 100, running(7)))
        .unwrap();

    clock.sleep(Duration::from_secs(60));
    let mut changed = def("daily", &["a/two"], true);
    changed.prompt = "other".to_owned();
    assert_eq!(store.put_workflow(alice, &changed).unwrap(), id);
    let got = store.workflow(alice, &name("daily")).unwrap().unwrap();
    assert_eq!(got.repos, [repo("a/two")]);
    assert_eq!(got.prompt, "other");
    assert_eq!(got.created_at, first.created_at);
    assert!(got.updated_at > first.updated_at);
    assert_eq!(
        got.state, first.state,
        "still enabled, so still since the same time"
    );
    assert_eq!(store.recent_workflow_runs(id, 50).unwrap().len(), 1);
}

#[test]
fn enabling_restarts_the_clock_and_disabling_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let alice = user(&store, "local:1");
    store
        .put_workflow(alice, &def("daily", &["a/one"], false))
        .unwrap();
    let state = |s: &Store| s.workflow(alice, &name("daily")).unwrap().unwrap().state;
    assert_eq!(state(&store), WorkflowState::Disabled);

    clock.sleep(Duration::from_secs(60));
    assert!(store
        .set_workflow_enabled(alice, &name("daily"), true)
        .unwrap());
    let since = store.now();
    assert_eq!(state(&store), WorkflowState::Enabled { since });

    clock.sleep(Duration::from_secs(60));
    assert!(store
        .set_workflow_enabled(alice, &name("daily"), true)
        .unwrap());
    assert_eq!(
        state(&store),
        WorkflowState::Enabled { since },
        "enabling twice changes nothing"
    );

    assert!(store
        .set_workflow_enabled(alice, &name("daily"), false)
        .unwrap());
    assert_eq!(state(&store), WorkflowState::Disabled);
    assert!(!store
        .set_workflow_enabled(alice, &name("absent"), true)
        .unwrap());
}

#[test]
fn only_the_owner_deletes_a_workflow_and_its_runs_go_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (alice, bob) = (user(&store, "local:1"), user(&store, "local:2"));
    let id = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    store
        .record_workflow_run(&run(id, "a/one", 100, skipped()))
        .unwrap();
    assert!(!store.delete_workflow(bob, &name("daily")).unwrap());
    assert!(store.workflow(alice, &name("daily")).unwrap().is_some());
    assert!(store.delete_workflow(alice, &name("daily")).unwrap());
    assert!(store.workflow(alice, &name("daily")).unwrap().is_none());
    assert!(store.recent_workflow_runs(id, 50).unwrap().is_empty());
    assert!(!store.delete_workflow(alice, &name("daily")).unwrap());
}

#[test]
fn a_firing_is_recorded_once_whatever_the_second_attempt_says() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    let started = run(id, "a/one", 100, running(7));
    assert!(store.record_workflow_run(&started).unwrap());
    let again = run(id, "a/one", 100, refused("full"));
    assert!(!store.record_workflow_run(&again).unwrap());
    assert_eq!(
        recorded_all(&store.recent_workflow_runs(id, 50).unwrap()),
        [started]
    );
}

#[test]
fn the_last_run_is_the_latest_due() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one", "a/two", "a/three"], true))
        .unwrap();
    let refusal = refused("no capacity");
    let newest = run(id, "a/one", 300, running(9));
    for r in [
        run(id, "a/one", 200, skipped()),
        newest.clone(),
        run(id, "a/one", 100, refusal.clone()),
        run(id, "a/two", 100, refusal),
    ] {
        assert!(store.record_workflow_run(&r).unwrap());
    }
    let last = store.last_workflow_run(id).unwrap();
    assert_eq!(recorded_all(&Vec::from_iter(last)), [newest]);
}

fn started(id: WorkflowId, r: &str, due: i64, box_id: i64) -> NewRun {
    run(id, r, due, running(box_id))
}

#[test]
fn a_timeout_is_stored_and_replaced_with_the_definition() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let mut d = def("daily", &["a/one"], true);
    d.timeout = TimeoutMinutes::new(45).unwrap();
    store.put_workflow(alice, &d).unwrap();
    let got = |s: &Store| s.workflow(alice, &name("daily")).unwrap().unwrap().timeout;
    assert_eq!(got(&store).minutes(), 45);
    d.timeout = TimeoutMinutes::new(720).unwrap();
    store.put_workflow(alice, &d).unwrap();
    assert_eq!(got(&store).minutes(), 720);
}

/// Ends the run at `key`, started at its only step, as Box `box_id` with `outcome`, at `at`.
pub(super) fn ends(
    store: &Store,
    key: moor_core::workflow::RunId,
    (box_id, outcome): (i64, &RunOutcome),
    at: Ts,
) -> bool {
    let (step, try_n) = single();
    let ended = (BoxId::new(box_id), outcome.clone());
    store
        .end_workflow_run(key, (step, try_n), ended, (&[], at))
        .unwrap()
}

/// The step and try of a run that was started at its only step.
pub(super) fn single() -> (&'static StepId, u8) {
    (&SINGLE, 1)
}

static SINGLE: std::sync::LazyLock<StepId> = std::sync::LazyLock::new(Plan::single_id);

#[test]
fn a_started_run_ends_once_as_finished_or_timed_out() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one", "a/two"], true))
        .unwrap();
    store
        .record_workflow_run(&started(id, "a/one", 100, 1))
        .unwrap();
    store
        .record_workflow_run(&started(id, "a/two", 100, 2))
        .unwrap();
    assert_eq!(store.open_workflow_runs(id).unwrap().len(), 2);

    let key = |r: &str| crate::testing::runs::run_at(&store, (id, &repo(r), Ts::from_millis(100)));
    let (k1, k2) = (key("a/one"), key("a/two"));
    let at = Ts::from_millis(900);
    let done = RunOutcome::Delivered {
        result: "opened a pull request".to_owned(),
        verdict: None,
    };
    let expired = RunOutcome::TimedOut {
        at: Plan::single_id(),
    };
    assert!(ends(&store, k1, (1, &done), at));
    assert!(
        !ends(&store, k1, (1, &expired), at),
        "a finished run is not expired afterwards"
    );
    assert!(ends(&store, k2, (2, &expired), at));
    assert!(store.open_workflow_runs(id).unwrap().is_empty());

    let mut last = store.recent_workflow_runs(id, 50).unwrap();
    last.sort_by_key(|r| r.repo.to_string());
    let outcomes: Vec<_> = last.into_iter().map(|r| r.state).collect();
    assert_eq!(
        outcomes,
        [
            RunState::Done(RunEnd::Ended {
                box_id: BoxId::new(1),
                ended_at: at,
                outcome: done,
            }),
            timed_out(2, 900),
        ]
    );
    let missing = moor_core::workflow::RunId::new(9_999);
    let outcome = RunOutcome::Delivered {
        result: String::new(),
        verdict: None,
    };
    assert!(!ends(&store, missing, (1, &outcome), at));
}

#[test]
fn open_runs_leave_out_skips_and_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    store
        .record_workflow_run(&started(id, "a/one", 100, 1))
        .unwrap();
    store
        .record_workflow_run(&run(id, "a/one", 200, skipped()))
        .unwrap();
    store
        .record_workflow_run(&run(id, "a/one", 300, refused("x")))
        .unwrap();
    let open = store.open_workflow_runs(id).unwrap();
    assert_eq!(recorded_all(&open), [started(id, "a/one", 100, 1)]);
}

#[test]
fn recent_runs_are_newest_first_and_limited() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one", "a/two"], true))
        .unwrap();
    for due in [100, 300, 200] {
        store
            .record_workflow_run(&started(id, "a/one", due, 1))
            .unwrap();
    }
    let dues: Vec<_> = store
        .recent_workflow_runs(id, 2)
        .unwrap()
        .iter()
        .map(|r| r.due_at.as_millis())
        .collect();
    assert_eq!(dues, [300, 200]);
}

fn state_of(store: &Store, owner: UserId) -> WorkflowState {
    store
        .workflow(owner, &name("daily"))
        .unwrap()
        .unwrap()
        .state
}

#[test]
fn enabling_counts_from_now_and_disabling_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let alice = user(&store, "local:1");
    store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    store
        .set_workflow_enabled(alice, &name("daily"), false)
        .unwrap();
    assert_eq!(state_of(&store, alice), WorkflowState::Disabled);

    clock.sleep(Duration::from_secs(60));
    store
        .set_workflow_enabled(alice, &name("daily"), true)
        .unwrap();
    assert_eq!(
        state_of(&store, alice),
        WorkflowState::Enabled { since: store.now() }
    );
}

#[test]
fn a_claimed_run_settles_once_and_counts_as_open_until_then() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    assert!(store
        .record_workflow_run(&run(id, "a/one", 100, RunState::Queued))
        .unwrap());
    assert_eq!(store.open_workflow_runs(id).unwrap().len(), 1);
    assert_eq!(store.starting_workflow_runs().unwrap().len(), 1);

    let key = crate::testing::runs::run_at(&store, (id, &repo("a/one"), Ts::from_millis(100)));
    let started = running(4);
    let at = Ts::from_millis(120);
    assert!(store.resolve_workflow_start(key, &started, at).unwrap());
    assert!(
        !store
            .resolve_workflow_start(key, &refused("late"), at)
            .unwrap(),
        "settled once"
    );
    assert!(store.starting_workflow_runs().unwrap().is_empty());
    let last = store.recent_workflow_runs(id, 50).unwrap();
    assert_eq!(last.first().map(|r| &r.state), Some(&started));
}

#[test]
fn ticked_workflows_lists_every_owners_enabled_ones() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (alice, bob) = (user(&store, "local:1"), user(&store, "local:2"));
    let a = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    store
        .put_workflow(alice, &def("off", &["a/one"], false))
        .unwrap();
    let b = store
        .put_workflow(bob, &def("daily", &["b/x"], true))
        .unwrap();
    let ids: Vec<_> = store
        .ticked_workflows()
        .unwrap()
        .iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(ids, [a, b]);
    assert_eq!(store.workflow_by_id(b).unwrap().unwrap().owner, bob);
}

#[test]
fn concurrent_settles_get_gapless_numbers_in_commit_order() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    std::thread::scope(|scope| {
        for t in 0..4_i64 {
            let store = &store;
            scope.spawn(move || {
                for n in 0..25_i64 {
                    let due = t * 1000 + n;
                    assert!(store
                        .record_workflow_run(&started(id, "a/one", due, 1))
                        .unwrap());
                }
            });
        }
    });
    let seqs: Vec<_> = store
        .workflow_changes(alice, Some(0), 1000, (None, None))
        .unwrap()
        .iter()
        .map(|c| c.seq)
        .collect();
    assert_eq!(seqs, (1..=100).collect::<Vec<_>>());
    assert!(!store
        .record_workflow_run(&started(id, "a/one", 0, 1))
        .unwrap());
    assert_eq!(
        store
            .workflow_changes(alice, Some(0), 1000, (None, None))
            .unwrap()
            .len(),
        100,
        "a repeated record takes no number"
    );
}
