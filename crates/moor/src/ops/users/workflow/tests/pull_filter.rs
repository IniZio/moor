//! `workflow-runs` filtered by pull request in the store query, with paging intact.

use moor_core::name::ForgeRepo;
use moor_core::request::WorkflowCmd;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{CommitSha, NewRun, PullRef, PullTarget, RunCause, WorkflowName};
use serde_json::Value;

use super::{cmd, daily, me, ok};
use crate::ops::users::run;
use crate::ops::users::tests::fixture;
use crate::testing::runs::skipped;

fn cause(repo: &str, number: u64, head: char) -> RunCause {
    let (owner, name) = repo.split_once('/').unwrap();
    RunCause::PullRequest {
        pull: PullRef {
            repo: ForgeRepo::new(owner, name),
            number,
            head_sha: CommitSha::parse(&head.to_string().repeat(40)).unwrap(),
        },
    }
}

fn due_ats(v: &Value) -> Vec<i64> {
    v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["due_at"].as_i64().unwrap())
        .collect()
}

#[test]
fn the_runs_stream_filters_by_pull_request_before_the_limit_and_pages() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    let store = f.cx.store().unwrap();
    let user = store.user_of(&who).unwrap().unwrap();
    let w = store
        .workflow(user, &WorkflowName::parse("daily").unwrap())
        .unwrap()
        .unwrap();
    let mut causes = vec![(100, cause("o/n", 7, 'a')), (200, cause("o/n", 8, 'a'))];
    // Many other runs between the matches, more than one page of them.
    causes.extend((0..5).map(|i| (300 + i, RunCause::Cron)));
    causes.push((400, cause("o/x", 7, 'a')));
    // A newer head of o/n#7: a run at the same head would be the same run.
    causes.push((500, cause("o/n", 7, 'b')));
    for (due, cause) in causes {
        let repo = cause
            .subject()
            .map_or_else(|| ForgeRepo::new("o", "n"), |p| p.repo.clone());
        let run = NewRun {
            workflow: w.id,
            repo,
            due_at: Ts::from_millis(due),
            state: skipped(),
            recorded_at: Ts::from_millis(due),
            plan: None,
            cause,
        };
        store.record_workflow_run(&run).unwrap();
    }
    let page = |after: Option<i64>, limit: u32, workflow: Option<&str>| {
        ok(
            &f,
            &who,
            cmd(WorkflowCmd::Runs {
                after,
                limit: Some(limit),
                workflow: workflow.map(str::to_owned),
                pull: Some(PullTarget::parse("o/n#7").unwrap()),
            }),
        )
    };
    let first = page(None, 1, None);
    assert_eq!(due_ats(&first), [100], "the limit counts only o/n#7's runs");
    let second = page(first["next"].as_i64(), 1, None);
    assert_eq!(
        due_ats(&second),
        [500],
        "the next page skips the other runs, o/x#7 included"
    );
    let third = page(second["next"].as_i64(), 1, None);
    assert_eq!(due_ats(&third), Vec::<i64>::new());
    assert_eq!(
        third["next"], second["next"],
        "an empty page keeps the cursor"
    );
    assert_eq!(due_ats(&page(None, 50, Some("daily"))), [100, 500]);
    assert_eq!(due_ats(&page(None, 50, Some("other"))), Vec::<i64>::new());
}

#[test]
fn a_pull_request_with_no_runs_reads_as_no_changes() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let none = run(
        &f.cx,
        &f.vault,
        &who,
        &cmd(WorkflowCmd::Runs {
            after: None,
            limit: None,
            workflow: None,
            pull: Some(PullTarget::parse("o/n#1").unwrap()),
        }),
    )
    .unwrap();
    assert_eq!(due_ats(&none), Vec::<i64>::new());
}
