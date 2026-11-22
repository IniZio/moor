//! `show` names the workflow run behind each Box and the node it runs on, on a list and on one Box.

use std::sync::Arc;

use moor_core::name::{ForgeRepo, NodeName};
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::workflow::{
    CommitSha, NewRun, Plan, PullRef, RunCause, RunState, TimeoutMinutes, WorkflowDef, WorkflowName,
};
use moor_core::BoxId;

use super::super::run;
use super::{add, cx, req};
use crate::store::Store;
use crate::testing::driver::FakeDriver;

/// A run of workflow `review` for pull request 65 of `acme/web`, which started Box `made`.
fn record_pull_run(store: &Store, user: UserId, made: BoxId) {
    let repo = ForgeRepo::parse("acme/web").unwrap();
    let def = WorkflowDef::new(
        WorkflowName::parse("review").unwrap(),
        None,
        vec![repo.clone()],
        "review it",
        (true, TimeoutMinutes::DEFAULT),
    )
    .unwrap();
    let flow = store.put_workflow(user, &def).unwrap();
    let pull = PullRef {
        repo: repo.clone(),
        number: 65,
        head_sha: CommitSha::parse("0123456789abcdef0123456789abcdef01234567").unwrap(),
    };
    let new = NewRun {
        workflow: flow,
        repo,
        due_at: Ts::from_millis(100),
        state: RunState::Running {
            box_id: made,
            at: Plan::single_id(),
            try_n: 1,
        },
        recorded_at: Ts::from_millis(105),
        plan: None,
        cause: RunCause::PullRequest { pull },
    };
    assert!(store.record_workflow_run(&new).unwrap());
}

#[test]
fn a_list_and_a_page_name_the_workflow_run_and_node_behind_a_box_and_an_unplaced_box_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let (made, placed, plain) = (add(&cx, "a"), add(&cx, "b"), add(&cx, "c"));
    let store = cx.store().unwrap();
    let (user, _) = store.enroll(&Identity::local(2000), None).unwrap();
    record_pull_run(store, user, made);
    store
        .set_box_node(placed, Some(&NodeName::parse("big").unwrap()))
        .unwrap();

    let listed = run(&cx, &req(None, None)).unwrap();
    let ties = |id: BoxId| {
        let all = listed.boxes.iter().flatten();
        all.into_iter()
            .find(|b| b.id == id.get())
            .unwrap()
            .ties
            .clone()
    };

    let flow = ties(made).workflow.unwrap();
    assert_eq!(
        (flow.name.as_str(), flow.pull.as_deref()),
        ("review", Some("acme/web#65"))
    );
    assert_eq!(ties(placed).machine.as_deref(), Some("big"));
    assert!(ties(plain).is_empty());
    let page = run(&cx, &req(Some("a"), None)).unwrap().boxed.unwrap();
    assert_eq!(page.ties, ties(made));
}
