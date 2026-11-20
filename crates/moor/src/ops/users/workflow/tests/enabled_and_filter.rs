//! `workflow-add` with `enabled` false, and `workflow-runs` filtered by workflow name.

use std::cell::RefCell;
use std::time::Duration;

use moor_core::name::ForgeRepo;
use moor_core::request::WorkflowCmd;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{NewRun, RunCause, WorkflowName};
use serde_json::{json, Value};

use super::{add, add_req, cmd, daily, me, ok, runs};
use crate::ops::users::tests::fixture;
use crate::ops::workflow::tick;
use crate::testing::runs::skipped;

#[test]
fn a_workflow_added_disabled_never_fires_on_a_due_cron_until_it_is_enabled() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut off = add_req("held", "0 9 * * *", "UTC");
    off.enabled = Some(false);
    let added = ok(&f, &who, add(off, None));
    assert_eq!(
        added["next_due"],
        json!(null),
        "a disabled workflow has no due time"
    );
    ok(&f, &who, add(add_req("live", "0 9 * * *", "UTC"), None));
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    let enabled: Vec<_> = listed["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["name"].clone(), w["enabled"].clone()))
        .collect();
    assert_eq!(
        enabled,
        [(json!("held"), json!(false)), (json!("live"), json!(true))]
    );

    f.cx.clock().sleep(Duration::from_secs(9 * 3600 + 1));
    let claimed = RefCell::new(Vec::new());
    tick(
        &f.cx,
        (
            &|job| claimed.borrow_mut().push(job.firing.workflow),
            &|_| {},
        ),
        &|| false,
    );
    let store = f.cx.store().unwrap();
    let user = store.user_of(&who).unwrap().unwrap();
    let live = store
        .workflow(user, &WorkflowName::parse("live").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.into_inner(),
        [live.id],
        "only the enabled workflow fires on the due cron"
    );
}

#[test]
fn the_runs_stream_filters_by_workflow_name_in_the_store_query() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    ok(
        &f,
        &who,
        add(add_req("weekly", "0 9 * * 1", "Europe/London"), None),
    );
    let store = f.cx.store().unwrap();
    let user = store.user_of(&who).unwrap().unwrap();
    for (name, due) in [("daily", 100), ("weekly", 200), ("daily", 300)] {
        let w = store
            .workflow(user, &WorkflowName::parse(name).unwrap())
            .unwrap()
            .unwrap();
        let run = NewRun {
            workflow: w.id,
            repo: ForgeRepo::new("o", "n"),
            due_at: Ts::from_millis(due),
            state: skipped(),
            recorded_at: Ts::from_millis(due),
            plan: None,
            cause: RunCause::Cron,
        };
        store.record_workflow_run(&run).unwrap();
    }
    let only = |name: &str, limit: Option<u32>| {
        ok(
            &f,
            &who,
            cmd(WorkflowCmd::Runs {
                after: None,
                limit,
                workflow: Some(name.to_owned()),
                pull: None,
            }),
        )
    };
    let names = |v: &Value| -> Vec<Value> {
        v["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["workflow"].clone())
            .collect()
    };
    assert_eq!(names(&only("weekly", None)), [json!("weekly")]);
    assert_eq!(
        names(&only("daily", None)),
        [json!("daily"), json!("daily")]
    );
    assert_eq!(
        names(&only("daily", Some(1))),
        [json!("daily")],
        "the limit counts only the named workflow's changes"
    );
    assert_eq!(names(&runs(&f, &who, None, None)).len(), 3);
    assert_eq!(names(&only("nobody", None)), Vec::<Value>::new());
}
