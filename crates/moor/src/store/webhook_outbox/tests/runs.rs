//! A run that waits, or ends `Failed`, `Incomplete`, `TimedOut` or `Refused`, writes one event for its
//! owner's webhooks in the move that changed its state; success and repeats write none.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index claimed rows"
)]

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::event::EVENT_KINDS;
use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::workflow::{
    NewRun, Plan, RunCause, RunOutcome, RunState, StepId, TimeoutMinutes, WorkflowDef, WorkflowId,
    WorkflowName, WorkflowSchedule,
};
use moor_core::{BoxId, Secret};
use serde_json::json;

use super::{insert, world, World};
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::runs::running;
use crate::vault::Vault;

const PROMPT: &str = "fix the security alerts";
const REPO: &str = "a/one";

pub(super) fn repo() -> ForgeRepo {
    ForgeRepo::parse(REPO).unwrap()
}

pub(super) fn step(s: &str) -> StepId {
    StepId::parse(s).unwrap()
}

pub(super) fn plan() -> Plan {
    Plan::parse(
        &json!({"steps": [
            {"id": "ask", "title": "Ask", "kind": "approve",
             "question": "ship the upgrade?", "ok": "ship"},
            {"id": "ship", "title": "Ship", "kind": "deliver",
             "instructions": "open the PR", "done_when": "the PR exists"}
        ]})
        .to_string(),
        PROMPT,
    )
    .unwrap()
}

fn def(with_plan: bool) -> WorkflowDef {
    let mut def = WorkflowDef::new(
        WorkflowName::parse("deps").unwrap(),
        Some(WorkflowSchedule::parse("0 9 * * *", "Europe/London").unwrap()),
        vec![repo()],
        PROMPT,
        (true, TimeoutMinutes::DEFAULT),
    )
    .unwrap();
    if with_plan {
        def.plan = Some(plan());
    }
    def
}

pub(super) fn new_run(id: WorkflowId, due: i64, state: RunState, plan: Option<Plan>) -> NewRun {
    NewRun {
        workflow: id,
        repo: repo(),
        due_at: Ts::from_millis(due),
        state,
        recorded_at: Ts::from_millis(due),
        plan,
        cause: RunCause::Cron,
    }
}

type Sent = Vec<(String, serde_json::Value)>;

impl World {
    /// A workflow and a Box of `owner`.
    pub(super) fn workflow(&self, owner: UserId, with_plan: bool) -> (WorkflowId, BoxId) {
        let id = self.store.put_workflow(owner, &def(with_plan)).unwrap();
        let boxed = self
            .store
            .apply(insert("deps-box").requester(owner))
            .unwrap()
            .box_id;
        (id, boxed)
    }

    /// Records the run due at `due` as running at its first step in `boxed`.
    pub(super) fn start(&self, id: WorkflowId, boxed: BoxId, due: i64, plan: Option<Plan>) {
        let run = new_run(id, due, running(boxed.get()), plan);
        assert!(self.store.record_workflow_run(&run).unwrap());
    }

    /// Every delivery of a run event, as (kind, payload), delivering all of them.
    fn sent(&self) -> Sent {
        let lease = Duration::from_millis(1);
        let mut out = Vec::new();
        loop {
            let got = self.store.claim_webhook_deliveries(10, lease).unwrap();
            if got.is_empty() {
                break;
            }
            for d in got {
                out.push((d.kind.clone(), serde_json::from_str(&d.payload).unwrap()));
                let ok = self.store.report_webhook_delivery(d.delivery, Ok(()));
                assert!(ok.unwrap());
            }
        }
        out.into_iter()
            .filter(|(k, _)| ["run_waiting", "run_failed"].contains(&k.as_str()))
            .collect()
    }
}

#[test]
#[allow(
    clippy::cognitive_complexity,
    reason = "one scenario, asserted step by step"
)]
fn an_incomplete_run_gets_exactly_one_delivery_however_often_the_move_repeats() {
    let w = world();
    let alice = w.user("local:2000");
    w.webhook(alice, EVENT_KINDS);
    let (id, boxed) = w.workflow(alice, false);
    w.start(id, boxed, 100, None);
    assert_eq!(w.sent(), []);
    let key = crate::testing::runs::run_at(&w.store, (id, &repo(), Ts::from_millis(100)));
    let on = Plan::single_id();
    let end = || {
        let outcome = RunOutcome::Incomplete {
            at: on.clone(),
            why: moor_core::workflow::Unfinished::Unreported,
        };
        let to = (boxed, outcome);
        let at = Ts::from_millis(900);
        w.store
            .end_workflow_run(key, (&on, 1), to, (&[], at))
            .unwrap()
    };
    assert!(end());
    assert!(!end());
    assert!(!end());
    let sent = w.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (kind, data) = &sent[0];
    assert_eq!(kind, "run_failed");
    assert_eq!(data["workflow"], "deps");
    assert_eq!(data["repo"], REPO);
    assert_eq!(data["outcome"], "incomplete");
    assert_eq!(data["step"], on.as_str());
    let run = data["run"].as_i64().unwrap();
    assert_eq!(data["link"], format!("/workflows/deps/runs/{run}"));
    assert!(data["reason"].as_str().unwrap().contains("reminder"));
}

#[test]
fn a_restart_after_the_move_does_not_send_it_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let open = || crate::store::db::testing::open_store(&path, clock.clone());
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let first: Store = open();
    let who = Identity::parse("local:2000").unwrap();
    let alice = first.enroll(&who, None).unwrap().0;
    let secret = Secret::new(b"k".to_vec());
    let kinds = ["run_failed".to_owned()];
    let hook = ("https://example.com/h", &kinds[..], None);
    let wh = first
        .add_user_webhook(&vault, alice, hook, &secret, 10)
        .unwrap()
        .unwrap();
    assert!(first
        .activate_user_webhook(&vault, alice, wh, None)
        .unwrap());
    let boxed = first.apply(insert("b").requester(alice)).unwrap().box_id;
    let id = first.put_workflow(alice, &def(false)).unwrap();
    let run = new_run(id, 100, running(boxed.get()), None);
    assert!(first.record_workflow_run(&run).unwrap());
    let key = crate::testing::runs::run_at(&first, (id, &repo(), Ts::from_millis(100)));
    let on = Plan::single_id();
    let ended = (boxed, RunOutcome::TimedOut { at: on.clone() });
    let at = Ts::from_millis(900);
    let moved = first.end_workflow_run(key, (&on, 1), ended.clone(), (&[], at));
    assert!(moved.unwrap());
    drop(first);
    let second = open();
    let again = second.end_workflow_run(key, (&on, 1), ended, (&[], at));
    assert!(!again.unwrap());
    assert_eq!(second.webhook_deliveries().unwrap().len(), 1);
}

#[test]
fn delivered_and_skipped_runs_send_nothing() {
    let w = world();
    let alice = w.user("local:2000");
    w.webhook(alice, EVENT_KINDS);
    let (id, boxed) = w.workflow(alice, false);
    w.start(id, boxed, 100, None);
    let on = Plan::single_id();
    let key = crate::testing::runs::run_at(&w.store, (id, &repo(), Ts::from_millis(100)));
    let ok = RunOutcome::Delivered {
        result: "done".to_owned(),
        verdict: None,
    };
    let at = Ts::from_millis(900);
    let moved = w
        .store
        .end_workflow_run(key, (&on, 1), (boxed, ok), (&[], at));
    assert!(moved.unwrap());
    let skip = new_run(id, 200, crate::testing::runs::skipped(), None);
    assert!(w.store.record_workflow_run(&skip).unwrap());
    assert_eq!(w.sent(), []);
}

#[test]
fn a_waiting_run_sends_one_delivery_with_its_question() {
    let w = world();
    let alice = w.user("local:2000");
    w.webhook(alice, EVENT_KINDS);
    let (id, boxed) = w.workflow(alice, true);
    w.start(id, boxed, 100, Some(plan()));
    let key = crate::testing::runs::run_at(&w.store, (id, &repo(), Ts::from_millis(100)));
    let (from, ask) = (Plan::single_id(), step("ask"));
    let wait = || {
        let at = Ts::from_millis(500);
        w.store
            .wait_workflow_run(key, ((&from, 1), (&ask, 1)), boxed, (&[], at))
            .unwrap()
    };
    assert!(wait());
    assert!(!wait());
    let sent = w.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (kind, data) = &sent[0];
    assert_eq!(kind, "run_waiting");
    assert_eq!(data["question"], "ship the upgrade?");
    assert_eq!(data["step"], "ask");
    assert_eq!(data["workflow"], "deps");
    let run = data["run"].as_i64().unwrap();
    assert_eq!(data["link"], format!("/workflows/deps/runs/{run}"));
}

#[test]
fn every_move_of_a_run_with_a_box_writes_run_moved_once_and_a_boxless_one_writes_none() {
    let w = world();
    let alice = w.user("local:2000");
    w.webhook(alice, &["run_moved"]);
    let (id, boxed) = w.workflow(alice, true);
    w.start(id, boxed, 100, Some(plan()));
    let key = crate::testing::runs::run_at(&w.store, (id, &repo(), Ts::from_millis(100)));
    let (from, ask) = (Plan::single_id(), step("ask"));
    let at = Ts::from_millis(500);
    let wait = || {
        w.store
            .wait_workflow_run(key, ((&from, 1), (&ask, 1)), boxed, (&[], at))
            .unwrap()
    };
    assert!(wait());
    assert!(!wait());
    let queued = new_run(id, 200, RunState::Queued, None);
    assert!(w.store.record_workflow_run(&queued).unwrap());
    let two = crate::testing::runs::run_at(&w.store, (id, &repo(), Ts::from_millis(200)));
    assert!(w.store.refuse_run(two, ("no node", true), at).unwrap());
    let lease = Duration::from_millis(1);
    let moved: Vec<_> = w
        .store
        .claim_webhook_deliveries(10, lease)
        .unwrap()
        .into_iter()
        .map(|d| {
            (
                d.kind,
                serde_json::from_str::<serde_json::Value>(&d.payload).unwrap(),
            )
        })
        .collect();
    let run = key.get();
    assert_eq!(
        moved,
        [(
            "run_moved".to_owned(),
            json!({"workflow": "deps", "run": run})
        )],
        "the wait moved once; the repeat and the run with no Box wrote nothing"
    );
}

#[test]
fn a_refusal_names_the_step_it_held_and_a_run_without_a_box_sends_nothing() {
    let w = world();
    let alice = w.user("local:2000");
    w.webhook(alice, &["run_failed"]);
    let (id, boxed) = w.workflow(alice, false);
    w.start(id, boxed, 100, None);
    let queued = new_run(id, 200, RunState::Queued, None);
    assert!(w.store.record_workflow_run(&queued).unwrap());
    let at = Ts::from_millis(900);
    let one = crate::testing::runs::run_at(&w.store, (id, &repo(), Ts::from_millis(100)));
    assert!(w
        .store
        .refuse_run(one, ("agent never started", false), at)
        .unwrap());
    let two = crate::testing::runs::run_at(&w.store, (id, &repo(), Ts::from_millis(200)));
    assert!(w.store.refuse_run(two, ("no node", true), at).unwrap());
    let sent = w.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].1["outcome"], "refused");
    assert_eq!(sent[0].1["reason"], "agent never started");
}
