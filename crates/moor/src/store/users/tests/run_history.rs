//! A run's history, and how each state of a run is stored and read back.

use moor_core::seam::clock::Ts;
use moor_core::workflow::{HistoryEntry, Plan, RunEnd, RunOutcome, RunPhase, RunState};
use moor_core::BoxId;
use moor_schema::tables::WorkflowRuns;
use sea_query::Query;

use super::workflow::{def, ends, repo, run, user};
use crate::store::db::DbExt as _;
use crate::store::tests::open;
use crate::testing::runs::{delivered, running, timed_out};

#[test]
fn every_move_of_a_run_appends_one_line_to_its_history() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    let key = (id, &repo("a/one"), Ts::from_millis(100));
    assert!(store
        .record_workflow_run(&run(id, "a/one", 100, RunState::Queued))
        .unwrap());
    assert!(!store
        .record_workflow_run(&run(id, "a/one", 100, RunState::Queued))
        .unwrap());
    assert!(store
        .resolve_workflow_start(key, &running(4), Ts::from_millis(110))
        .unwrap());
    assert!(!store
        .resolve_workflow_start(key, &running(5), Ts::from_millis(115))
        .unwrap());
    let outcome = RunOutcome::Delivered {
        result: "done".to_owned(),
    };
    assert!(ends(&store, key, (4, &outcome), Ts::from_millis(900)));
    assert!(!ends(&store, key, (4, &outcome), Ts::from_millis(950)));

    let run_id = store
        .recent_workflow_runs(id, 50)
        .unwrap()
        .first()
        .unwrap()
        .id;
    let lines = store.run_history(run_id).unwrap();
    let want = [
        (100 + 5, HistoryEntry::Queued),
        (
            110,
            HistoryEntry::Entered {
                step: Plan::single_id(),
                try_n: 1,
            },
        ),
        (
            900,
            HistoryEntry::Ended {
                phase_before: RunPhase::Running,
                outcome,
            },
        ),
    ];
    let got: Vec<_> = lines
        .iter()
        .map(|l| {
            (
                l.at.as_millis(),
                serde_json::from_value(l.entry.clone()).unwrap(),
            )
        })
        .collect();
    assert_eq!(got, want);
    assert_eq!(lines.iter().map(|l| l.n).collect::<Vec<_>>(), [1, 2, 3]);
}

#[test]
fn a_run_reads_back_with_its_step_result_and_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    let at = Plan::single_id();
    let states = [
        RunState::Waiting {
            box_id: BoxId::new(1),
            at: at.clone(),
            try_n: 2,
            since: Ts::from_millis(77),
        },
        RunState::Done(RunEnd::Ended {
            box_id: BoxId::new(1),
            ended_at: Ts::from_millis(9),
            outcome: RunOutcome::Failed {
                at: at.clone(),
                reason: "tests red".to_owned(),
            },
        }),
        RunState::Done(RunEnd::Ended {
            box_id: BoxId::new(1),
            ended_at: Ts::from_millis(9),
            outcome: RunOutcome::Incomplete { at: at.clone() },
        }),
        RunState::Done(RunEnd::Ended {
            box_id: BoxId::new(1),
            ended_at: Ts::from_millis(9),
            outcome: RunOutcome::Declined { at },
        }),
        delivered(1, 9),
    ];
    for (due, state) in (1..).zip(states.iter()) {
        let new = run(id, "a/one", due, state.clone());
        assert!(store.record_workflow_run(&new).unwrap());
        let got = store.recent_workflow_runs(id, 1).unwrap();
        assert_eq!(got.first().map(|r| &r.state), Some(state));
    }
    assert_eq!(
        store.open_workflow_runs(id).unwrap().len(),
        1,
        "a waiting run is open"
    );
}

#[test]
fn a_run_from_before_plans_reads_at_the_single_step() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = user(&store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one"], true))
        .unwrap();
    let mut insert = Query::insert();
    insert.into_table(WorkflowRuns::Table).columns([
        WorkflowRuns::Workflow,
        WorkflowRuns::Repo,
        WorkflowRuns::DueAt,
        WorkflowRuns::Box,
        WorkflowRuns::Outcome,
        WorkflowRuns::EndedAt,
        WorkflowRuns::RecordedAt,
    ]);
    for (due, outcome, ended) in [
        (1_i64, "started", None),
        (2, "timed_out", Some(5_i64)),
        (3, "finished", Some(6)),
    ] {
        insert.values_panic([
            id.get().into(),
            "a/one".into(),
            due.into(),
            3_i64.into(),
            outcome.into(),
            ended.into(),
            due.into(),
        ]);
    }
    store.writer().run(insert).unwrap();
    let mut got = store.recent_workflow_runs(id, 3).unwrap();
    got.reverse();
    let states: Vec<_> = got.into_iter().map(|r| r.state).collect();
    assert_eq!(states, [running(3), timed_out(3, 5), delivered(3, 6)]);
}
