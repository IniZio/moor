//! A workflow with a plan: its run freezes the plan, its Box is told the first step, and a quiet
//! agent is sent on to the next step.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "test assertions index JSON values and panic on a wrong shape"
)]

use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::workflow::{
    Plan, RunEnd, RunOutcome, RunState, TimeoutMinutes, WorkflowDef, WorkflowId, WorkflowName,
    WorkflowSchedule,
};
use serde_json::json;

use super::{repo, rig, Rig, FIRST_DUE};
use crate::testing::cx::REQUESTER_USER;

pub(super) const PROMPT: &str = "keep the dependencies current";

fn two_steps() -> Plan {
    Plan::parse(
        &json!({"steps": [
            {"id": "audit", "title": "Audit", "kind": "work",
             "instructions": "list the outdated packages", "done_when": "the list is written",
             "evidence": ["outdated.md"], "ok": "ship"},
            {"id": "ship", "title": "Ship", "kind": "deliver",
             "instructions": "open the pull request", "done_when": "the PR exists"}
        ]})
        .to_string(),
        PROMPT,
    )
    .unwrap()
}

impl Rig {
    /// Stores the daily workflow with `plan`.
    pub(super) fn put_planned(&self, plan: Option<Plan>) -> WorkflowId {
        let mut def = WorkflowDef::new(
            WorkflowName::parse("deps").unwrap(),
            Some(WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap()),
            vec![repo()],
            PROMPT,
            (true, TimeoutMinutes::DEFAULT),
        )
        .unwrap();
        def.plan = plan;
        self.cx
            .store()
            .unwrap()
            .put_workflow(REQUESTER_USER, &def)
            .unwrap()
    }

    /// The `kind` of each history line of the newest run.
    pub(super) fn kinds(&self, id: WorkflowId) -> Vec<String> {
        let run = self.latest(id);
        self.cx
            .store()
            .unwrap()
            .run_history(run.id)
            .unwrap()
            .iter()
            .map(|l| l.entry["kind"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The agent, working on what the steer sent, goes quiet.
    pub(super) fn quiet_after_work(&self, id: WorkflowId) {
        self.pause(1_000);
        self.cx
            .store()
            .unwrap()
            .apply(Decision::new(self.latest_box(id)).session(SessionInput::AgentQuiet))
            .unwrap();
    }

    pub(super) fn pause(&self, ms: i64) {
        self.advance_to(self.cx.clock().now().as_millis() + ms);
    }
}

pub(super) fn step_of(state: &RunState) -> (String, u8) {
    match state {
        RunState::Running { at, try_n, .. } | RunState::Waiting { at, try_n, .. } => {
            (at.to_string(), *try_n)
        }
        other => panic!("not running: {other:?}"),
    }
}

#[test]
fn a_planned_run_freezes_its_plan_and_its_box_is_told_the_first_step() {
    let r = rig();
    r.answer_repo();
    let id = r.put_planned(Some(two_steps()));
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    let run = r.latest(id);
    assert_eq!(step_of(&run.state), ("audit".to_owned(), 1));
    let store = r.cx.store().unwrap();
    assert_eq!(store.run_plan(run.id, "anything").unwrap(), two_steps());
    let row = store.box_row(r.latest_box(id)).unwrap().unwrap();
    let prompt = row.prompt.unwrap();
    assert!(prompt.starts_with(PROMPT), "{prompt}");
    assert!(prompt.contains("plan of 2 steps"), "{prompt}");
    assert!(
        prompt.contains("Current step: audit, \"Audit\""),
        "{prompt}"
    );
    assert!(
        prompt.contains("Done when: the list is written"),
        "{prompt}"
    );
    assert!(prompt.contains("Evidence: outdated.md"), "{prompt}");
    assert!(!prompt.contains("open the pull request"), "{prompt}");
}

#[test]
fn an_edit_never_changes_a_run_in_flight() {
    let r = rig();
    r.answer_repo();
    let id = r.put_planned(Some(two_steps()));
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let run = r.latest(id);
    r.put_planned(None);
    let store = r.cx.store().unwrap();
    assert_eq!(store.run_plan(run.id, PROMPT).unwrap(), two_steps());
}

#[test]
fn a_workflow_without_a_plan_gets_its_bare_prompt_and_freezes_none() {
    let r = rig();
    r.answer_repo();
    let id = r.put_planned(None);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let run = r.latest(id);
    let store = r.cx.store().unwrap();
    let row = store.box_row(r.latest_box(id)).unwrap().unwrap();
    assert_eq!(row.prompt.as_deref(), Some(PROMPT));
    assert!(store.run_plan(run.id, PROMPT).unwrap().is_single_of(PROMPT));
}

#[test]
fn a_two_step_plan_advances_on_a_report_and_delivers_with_the_deliver_note() {
    let r = rig();
    r.answer_repo();
    let id = r.put_planned(Some(two_steps()));
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let box_id = r.latest_box(id);
    let store = r.cx.store().unwrap();
    store
        .apply(Decision::new(box_id).session(SessionInput::AgentQuiet))
        .unwrap();
    r.pause(3 * 60_000);

    let told = r.report_current(id);
    assert!(!told.error, "{told:?}");
    assert_eq!(step_of(&r.latest(id).state), ("ship".to_owned(), 1));
    assert_eq!(
        r.kinds(id),
        ["queued", "entered", "reported", "entered", "sent"]
    );

    // The reply is on its way and the Box still shows the quiet before the first report. That
    // reading says nothing about the second step, so no pass may remind or end the run.
    r.pause(3 * 60_000);
    r.pass();
    r.pass();
    assert_eq!(step_of(&r.latest(id).state), ("ship".to_owned(), 1));
    assert_eq!(
        r.kinds(id),
        ["queued", "entered", "reported", "entered", "sent"]
    );

    let told = r.report_current(id);
    assert_eq!(told.reply, "Done.");
    let run = r.latest(id);
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Delivered { result },
        ..
    }) = run.state
    else {
        panic!("{:?}", run.state);
    };
    assert_eq!(result, "all done");
    assert_eq!(
        r.kinds(id),
        ["queued", "entered", "reported", "entered", "sent", "reported", "ended"]
    );
    let entries = store.run_history(run.id).unwrap();
    assert_eq!(entries[1].entry["step"], "audit");
    assert_eq!(entries[3].entry["step"], "ship");
    assert_eq!(entries[3].entry["try_n"], 1);
    assert_eq!(entries[4].entry["step"], "ship");
}

#[test]
fn a_loop_runs_out_of_tries_and_takes_its_fail_edge() {
    let plan = Plan::parse(
        &json!({"steps": [
            {"id": "fix", "title": "Fix", "kind": "work", "max_tries": 2,
             "instructions": "fix it", "done_when": "fixed", "evidence": ["fix.md"],
             "ok": "fix", "fail": "ship"},
            {"id": "ship", "title": "Ship", "kind": "deliver",
             "instructions": "ship it", "done_when": "shipped"}
        ]})
        .to_string(),
        PROMPT,
    )
    .unwrap();
    let r = rig();
    r.answer_repo();
    let id = r.put_planned(Some(plan));
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    assert!(!r.report_current(id).error);
    assert_eq!(step_of(&r.latest(id).state), ("fix".to_owned(), 2));

    assert!(!r.report_current(id).error);
    assert_eq!(step_of(&r.latest(id).state), ("ship".to_owned(), 1));
    let kinds = r.kinds(id);
    assert!(kinds.contains(&"exhausted".to_owned()), "{kinds:?}");
}

#[test]
fn a_box_that_is_gone_before_the_last_step_ends_the_run_incomplete() {
    let r = rig();
    r.answer_repo();
    let id = r.put_planned(Some(two_steps()));
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let box_id = r.latest_box(id);
    let closed = moor_core::domain::box_state::BoxLifecycleInput::Close { reason: None };
    r.cx.store()
        .unwrap()
        .apply(
            Decision::new(box_id)
                .lifecycle(closed)
                .session(SessionInput::End),
        )
        .unwrap();
    r.pass();
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Incomplete { at },
        ..
    }) = r.latest(id).state
    else {
        panic!("{:?}", r.latest(id).state);
    };
    assert_eq!(at.as_str(), "audit");
}

/// A rig whose controller gives its Boxes no `moor` MCP server, so no agent can call
/// `report_step`.
fn unreportable() -> Rig {
    let mut r = rig();
    let mut config = r.cx.config().clone();
    config.moor_service = None;
    r.cx = r.cx.with_config(config);
    r
}

#[test]
fn a_planned_run_whose_agent_could_not_report_a_step_is_refused_before_its_box() {
    let r = unreportable();
    r.answer_repo();
    let id = r.put_planned(Some(two_steps()));
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let RunState::Done(RunEnd::Refused { detail }) = r.latest(id).state else {
        panic!("{:?}", r.latest(id).state);
    };
    assert!(detail.contains("report_step"), "{detail}");
    assert_eq!(r.open_boxes(), 0);
}

#[test]
fn a_workflow_without_a_plan_starts_where_no_step_can_be_reported() {
    let r = unreportable();
    r.answer_repo();
    let id = r.put_planned(None);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    assert!(
        matches!(r.latest(id).state, RunState::Running { .. }),
        "{:?}",
        r.latest(id).state
    );
}

#[test]
fn a_planned_run_with_no_front_to_report_through_is_refused_before_its_box() {
    let mut r = rig();
    r.cx =
        r.cx.with_env(|e| e.with_front_step_reports(std::collections::BTreeSet::new()));
    r.answer_repo();
    let id = r.put_planned(Some(two_steps()));
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let RunState::Done(RunEnd::Refused { detail }) = r.latest(id).state else {
        panic!("{:?}", r.latest(id).state);
    };
    assert!(detail.contains("step_reports"), "{detail}");
    assert_eq!(r.open_boxes(), 0);
}

#[test]
fn a_step_report_restarts_the_stall_clock_of_a_long_turn() {
    use moor_core::activity::{Activity, AgentState, Life, Source, Status, Term};
    use moor_core::seam::clock::Ts;

    use crate::ops::common::activity::{settle, PaneLiveness, Reading};

    let r = rig();
    r.answer_repo();
    let id = r.put_planned(Some(two_steps()));
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let turn_began = r.cx.clock().now();
    r.pause(9 * 60_000);
    assert!(!r.report_current(id).error);
    r.pause(2 * 60_000);
    let working = Activity {
        state: AgentState::Working,
        source: Source::Status,
    };
    let mut reading = Reading::new(Some(working), PaneLiveness::Live, false, false);
    reading.term = Some(Term {
        life: Life::Live,
        status: Some(Status::Working { doing: None }),
        status_at: Some(Ts::from_millis(turn_began.as_millis())),
        turn: 0,
    });
    let store = r.cx.store().unwrap();
    let row = store.box_row(r.latest_box(id)).unwrap().unwrap();
    settle(&r.cx, store, &row, &reading).unwrap();
    let events = store.events(row.id, 200).unwrap();
    assert!(
        events.iter().all(|e| e.kind != "agent_stalled"),
        "the report two minutes ago is activity"
    );
}
