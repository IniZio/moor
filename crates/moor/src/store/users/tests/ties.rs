//! The workflow run behind each Box and the node it runs on, read for many Boxes in one query.

use moor_core::name::NodeName;
use moor_core::workflow::{CommitSha, PullRef, RunCause, RunState};
use moor_core::BoxId;

use super::workflow::{def, name, repo, run, user};
use crate::store::tests::{new_box, open};
use crate::testing::runs::running;

#[test]
fn each_box_names_the_workflow_of_its_latest_run_and_the_pull_request_it_was_for() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let owner = user(&store, "local:1");
    let review = store
        .put_workflow(owner, &def("review", &["o/n"], true))
        .unwrap();
    let nightly = store
        .put_workflow(owner, &def("nightly", &["o/n"], true))
        .unwrap();
    let mut for_pull = run(review, "o/n", 100, running(1));
    for_pull.cause = RunCause::PullRequest {
        pull: PullRef {
            repo: repo("o/n"),
            number: 65,
            head_sha: CommitSha::parse("0123456789abcdef0123456789abcdef01234567").unwrap(),
        },
    };
    assert!(store.record_workflow_run(&for_pull).unwrap());
    assert!(store
        .record_workflow_run(&run(nightly, "o/n", 200, running(2)))
        .unwrap());
    assert!(store
        .record_workflow_run(&run(nightly, "o/n", 300, RunState::Queued))
        .unwrap());
    assert!(store
        .record_workflow_run(&run(nightly, "o/n", 400, running(1)))
        .unwrap());

    let ties = store
        .ties_of(&[BoxId::new(1), BoxId::new(2), BoxId::new(3)])
        .unwrap();

    let named = |id: i64| ties.get(&BoxId::new(id)).and_then(|t| t.workflow.clone());
    assert_eq!(
        named(2).map(|w| (w.name, w.pull)),
        Some(("nightly".into(), None))
    );
    assert_eq!(
        named(1).map(|w| (w.name, w.pull)),
        Some(("nightly".into(), None)),
        "a Box a later run reused names that run's workflow"
    );
    assert!(
        !ties.contains_key(&BoxId::new(3)),
        "a Box no run made has no ties"
    );
    assert!(store.ties_of(&[]).unwrap().is_empty());
}

#[test]
fn a_run_for_a_pull_request_names_it_and_a_deleted_workflow_names_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let owner = user(&store, "local:1");
    let review = store
        .put_workflow(owner, &def("review", &["o/n"], true))
        .unwrap();
    let gone = store
        .put_workflow(owner, &def("gone", &["o/n"], true))
        .unwrap();
    let mut for_pull = run(review, "o/n", 100, running(1));
    for_pull.cause = RunCause::PullRequest {
        pull: PullRef {
            repo: repo("o/n"),
            number: 65,
            head_sha: CommitSha::parse("0123456789abcdef0123456789abcdef01234567").unwrap(),
        },
    };
    assert!(store.record_workflow_run(&for_pull).unwrap());
    assert!(store
        .record_workflow_run(&run(gone, "o/n", 100, running(2)))
        .unwrap());
    assert!(store.delete_workflow(owner, &name("gone")).unwrap());

    let ties = store.ties_of(&[BoxId::new(1), BoxId::new(2)]).unwrap();

    let review = ties
        .get(&BoxId::new(1))
        .and_then(|t| t.workflow.clone())
        .unwrap();
    assert_eq!(
        (review.name.as_str(), review.pull.as_deref()),
        ("review", Some("o/n#65"))
    );
    assert!(!ties.contains_key(&BoxId::new(2)));
}

#[test]
fn a_box_on_a_node_names_it_as_its_machine_and_an_unplaced_box_names_none() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (placed, unplaced) = (new_box(&store, "a"), new_box(&store, "b"));
    let node = NodeName::parse("big").unwrap();
    store.set_box_node(placed, Some(&node)).unwrap();

    let ties = store.ties_of(&[placed, unplaced]).unwrap();

    assert_eq!(
        ties.get(&placed).and_then(|t| t.machine.as_deref()),
        Some("big")
    );
    assert!(!ties.contains_key(&unplaced));
}
