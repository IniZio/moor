//! The claim key of a run for a pull request: one run per workflow, repo, pull request and head.

use moor_core::seam::clock::Ts;
use moor_core::workflow::{CommitSha, PullRef, RunCause, RunEnd, RunState, WorkflowRun};

use super::workflow::{def, repo, run, user};
use crate::store::tests::open;

/// The head of pull request 65 the claims use, and a later one.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const NEWER: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn pull(sha: &str) -> PullRef {
    PullRef {
        repo: repo("o/n"),
        number: 65,
        head_sha: CommitSha::parse(sha).unwrap(),
    }
}

#[test]
fn a_pull_request_at_one_head_is_claimed_once_and_a_new_head_claims_again() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let owner = user(&store, "local:1");
    let id = store
        .put_workflow(owner, &def("review", &["o/n"], true))
        .unwrap();
    let claim = |due: i64, sha: &str| {
        let mut new = run(id, "o/n", due, RunState::Queued);
        new.cause = RunCause::PullRequest { pull: pull(sha) };
        store.record_workflow_run(&new).unwrap()
    };
    assert!(claim(1_000, HEAD), "the first delivery claims");
    assert!(
        !claim(2_000, HEAD),
        "a redelivery of the same head records nothing"
    );
    assert!(claim(3_000, NEWER), "a push claims its new head");
    let runs: Vec<WorkflowRun> = store.recent_workflow_runs(id, 10).unwrap();
    let heads: Vec<(i64, RunCause)> = runs
        .iter()
        .map(|r| (r.due_at.as_millis(), r.cause.clone()))
        .collect();
    assert_eq!(
        heads,
        [
            (3_000, RunCause::PullRequest { pull: pull(NEWER) }),
            (1_000, RunCause::PullRequest { pull: pull(HEAD) }),
        ]
    );
}

#[test]
fn a_skipped_fork_and_a_manual_run_at_a_claimed_head_still_record() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let owner = user(&store, "local:1");
    let id = store
        .put_workflow(owner, &def("review", &["o/n"], true))
        .unwrap();
    let record = |due: i64, state: RunState, cause: RunCause| {
        let mut new = run(id, "o/n", due, state);
        new.cause = cause;
        store.record_workflow_run(&new).unwrap()
    };
    let fork = RunState::Done(RunEnd::SkippedFork {
        author: "someone".to_owned(),
    });
    assert!(record(
        1_000,
        fork.clone(),
        RunCause::PullRequest { pull: pull(HEAD) }
    ));
    assert!(record(
        2_000,
        RunState::Queued,
        RunCause::PullRequest { pull: pull(HEAD) }
    ));
    assert!(record(
        3_000,
        RunState::Queued,
        RunCause::Manual {
            pull: Some(pull(HEAD))
        }
    ));
    let oldest = store.recent_workflow_runs(id, 10).unwrap().pop().unwrap();
    assert_eq!(oldest.state, fork, "the skip reads back with its author");
    assert_eq!(oldest.due_at, Ts::from_millis(1_000));
}
