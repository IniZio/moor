//! A workflow's stored plan and revision, and a run's frozen plan and step pointer.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "test assertions index JSON values and panic on a wrong shape"
)]

use moor_core::seam::clock::Ts;
use moor_core::workflow::{HistoryEntry, Plan, RunOutcome, RunState, StepId};
use moor_core::BoxId;
use serde_json::json;

use super::workflow::{def, repo, run, user};
use crate::store::tests::open;
use crate::store::Put;
use crate::testing::runs::running;

const PROMPT: &str = "fix the security alerts";

fn plan() -> Plan {
    Plan::parse(
        &json!({"steps": [
            {"id": "audit", "title": "Audit", "kind": "work", "instructions": "list them",
             "done_when": "listed", "evidence": ["alerts.md"], "ok": "ship"},
            {"id": "ship", "title": "Ship", "kind": "deliver",
             "instructions": "open the PR", "done_when": "the PR exists"}
        ]})
        .to_string(),
        PROMPT,
    )
    .unwrap()
}

fn id(s: &str) -> StepId {
    StepId::parse(s).unwrap()
}

#[test]
fn a_plan_is_stored_read_back_and_dropped_when_the_workflow_is_put_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let mut d = def("daily", &["a/one"], true);
    store.put_workflow(alice, &d).unwrap();
    let read = |s: &crate::store::Store| s.workflow(alice, &d.name).unwrap().unwrap();
    assert!(read(&store).plan.is_single_of(PROMPT));

    d.plan = Some(plan());
    store.put_workflow(alice, &d).unwrap();
    assert_eq!(read(&store).plan, plan());

    d.plan = None;
    store.put_workflow(alice, &d).unwrap();
    assert!(read(&store).plan.is_single_of(PROMPT));
}

#[test]
fn a_put_writes_only_at_the_revision_the_writer_read() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let d = def("daily", &["a/one"], true);
    let at = |expected| store.put_workflow_at(alice, &d, expected, false).unwrap();

    assert!(matches!(at(Some(0)), Put::Stale { current: None }));
    assert!(matches!(at(None), Put::Written { revision: 0, .. }));
    assert!(matches!(at(Some(0)), Put::Written { revision: 1, .. }));
    assert!(matches!(at(Some(0)), Put::Stale { current: Some(1) }));
    assert!(matches!(at(Some(1)), Put::Written { revision: 2, .. }));
    assert_eq!(store.workflow(alice, &d.name).unwrap().unwrap().revision, 2);
}

#[test]
fn a_run_freezes_the_plan_it_was_claimed_with() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let mut d = def("daily", &["a/one"], true);
    d.plan = Some(plan());
    let wid = store.put_workflow(alice, &d).unwrap();
    let mut claim = run(wid, "a/one", 100, RunState::Queued);
    claim.plan = d.plan.clone();
    assert!(store.record_workflow_run(&claim).unwrap());
    assert!(store
        .record_workflow_run(&run(wid, "a/one", 200, RunState::Queued))
        .unwrap());

    d.plan = None;
    store.put_workflow(alice, &d).unwrap();
    let runs = store.recent_workflow_runs(wid, 2).unwrap();
    assert_eq!(store.run_plan(runs[1].id, PROMPT).unwrap(), plan());
    assert!(store
        .run_plan(runs[0].id, PROMPT)
        .unwrap()
        .is_single_of(PROMPT));
}

#[test]
fn the_pointer_moves_once_from_exactly_the_step_and_try_it_was_read_at() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let wid = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    store
        .record_workflow_run(&run(wid, "a/one", 100, RunState::Queued))
        .unwrap();
    let key = crate::testing::runs::run_at(&store, (wid, &repo("a/one"), Ts::from_millis(100)));
    let first = Plan::single_id();
    let mut state = running(4);
    let RunState::Running { at, .. } = &mut state else {
        unreachable!()
    };
    *at = first.clone();
    assert!(store
        .resolve_workflow_start(key, &state, Ts::from_millis(110))
        .unwrap());

    let lines = [
        HistoryEntry::Entered {
            step: id("next"),
            try_n: 1,
        },
        HistoryEntry::Sent {
            step: id("next"),
            try_n: 1,
        },
    ];
    let moved = |from: (&StepId, u8)| {
        store
            .step_workflow_run(
                key,
                (from, (&id("next"), 1)),
                (&lines, Ts::from_millis(200)),
            )
            .unwrap()
    };
    assert!(!moved((&first, 2)), "a try that is not the current one");
    assert!(
        !moved((&id("other"), 1)),
        "a step that is not the current one"
    );
    assert!(moved((&first, 1)));
    assert!(!moved((&first, 1)), "the same move twice applies once");

    let run_row = store.recent_workflow_runs(wid, 50).unwrap().remove(0);
    let RunState::Running { at, try_n, .. } = &run_row.state else {
        panic!("{:?}", run_row.state);
    };
    assert_eq!((at.as_str(), *try_n), ("next", 1));
    let kinds: Vec<_> = store
        .run_entries(run_row.id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    assert_eq!(&kinds[2..], &lines);
}

#[test]
fn a_run_ends_only_from_exactly_the_step_and_try_it_was_read_at_with_its_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let wid = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    store
        .record_workflow_run(&run(wid, "a/one", 100, RunState::Queued))
        .unwrap();
    let key = crate::testing::runs::run_at(&store, (wid, &repo("a/one"), Ts::from_millis(100)));
    let first = Plan::single_id();
    assert!(store
        .resolve_workflow_start(key, &running(4), Ts::from_millis(110))
        .unwrap());
    let next = id("next");
    assert!(store
        .step_workflow_run(key, ((&first, 1), (&next, 1)), (&[], Ts::from_millis(150)),)
        .unwrap());

    let line = [HistoryEntry::Reported {
        step: next.clone(),
        try_n: 1,
        report: moor_core::workflow::StepReport {
            step: next.clone(),
            result: moor_core::workflow::StepResult::Ok,
            evidence: Vec::new(),
            note: String::new(),
        },
    }];
    let outcome = RunOutcome::Incomplete {
        at: first.clone(),
        why: moor_core::workflow::Unfinished::Unreported,
    };
    let end = |on: (&StepId, u8), lines: &[HistoryEntry]| {
        store
            .end_workflow_run(
                key,
                on,
                (BoxId::new(4), outcome.clone()),
                (lines, Ts::from_millis(200)),
            )
            .unwrap()
    };
    assert!(!end((&first, 1), &line), "a step the run has left");
    assert!(!end((&next, 2), &line), "a try the run is not at");
    let run_row = store.recent_workflow_runs(wid, 50).unwrap().remove(0);
    assert_eq!(store.run_history(run_row.id).unwrap().len(), 2);
    assert!(end((&next, 1), &line));
    let kinds: Vec<_> = store
        .run_history(run_row.id)
        .unwrap()
        .into_iter()
        .map(|l| l.entry["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, ["queued", "entered", "reported", "ended"]);
}
