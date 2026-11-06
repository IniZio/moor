//! Refusing a run that is overdue, and when a Box's agent first started.

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::workflow::{WorkflowId, WorkflowOutcome};
use moor_core::BoxId;

use super::workflow::{def, repo, run, user};
use crate::store::tests::open;

/// A workflow of two repos, each with a claimed run due at 100; `a/two` is then started.
fn two_claims(store: &crate::store::Store) -> (UserId, WorkflowId) {
    let alice = user(store, "local:1");
    let id = store
        .put_workflow(alice, &def("daily", &["a/one", "a/two"], true))
        .unwrap();
    for r in ["a/one", "a/two"] {
        let claimed = run(id, r, 100, WorkflowOutcome::Starting);
        assert!(store.record_workflow_run(&claimed).unwrap());
    }
    let started = WorkflowOutcome::Started {
        box_id: BoxId::new(4),
    };
    let two = (id, &repo("a/two"), Ts::from_millis(100));
    assert!(store.resolve_workflow_start(two, &started).unwrap());
    (alice, id)
}

#[test]
fn an_overdue_run_is_refused_only_in_the_state_it_was_seen_in() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (_, id) = two_claims(&store);
    let one = (id, &repo("a/one"), Ts::from_millis(100));
    let two = (id, &repo("a/two"), Ts::from_millis(100));
    assert!(!store.refuse_run(one, ("late", false)).unwrap());
    assert!(store.refuse_run(one, ("late", true)).unwrap());
    assert!(!store.refuse_run(one, ("again", true)).unwrap());
    assert!(!store.refuse_run(two, ("late", true)).unwrap());
    assert!(store.refuse_run(two, ("late", false)).unwrap());
    let want = WorkflowOutcome::Refused {
        detail: "late".to_owned(),
    };
    for r in ["a/one", "a/two"] {
        let latest = store.recent_workflow_runs(id, &repo(r), 1).unwrap();
        assert_eq!(latest.first().map(|l| &l.outcome), Some(&want));
    }
}

#[test]
fn a_start_that_returns_after_its_run_was_refused_changes_nothing_and_the_stream_shows_the_refusal()
{
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (alice, id) = two_claims(&store);
    let before = store.workflow_changes(alice, 0, 50).unwrap();
    let one = (id, &repo("a/one"), Ts::from_millis(100));
    assert!(store.refuse_run(one, ("late", true)).unwrap());
    let started = WorkflowOutcome::Started {
        box_id: BoxId::new(5),
    };
    assert!(!store.resolve_workflow_start(one, &started).unwrap());
    let after = store.workflow_changes(alice, 0, 50).unwrap();
    assert!(after.last().map(|c| c.seq) > before.last().map(|c| c.seq));
}

#[test]
fn a_box_that_never_launched_an_agent_has_no_first_launch() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert_eq!(store.first_launch_at(BoxId::new(1)).unwrap(), None);
}
