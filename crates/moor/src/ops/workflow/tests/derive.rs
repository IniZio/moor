//! moor makes a workflow's plan from its prompt in a short Box, and stores only that Box's plan.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "a test indexes what it just read and walks one scenario step by step"
)]

use std::cell::RefCell;

use moor_core::gate::{GateDoc, Gates};
use moor_core::workflow::{is_draft_prompt, PlanDraft, WorkflowDef, WorkflowId};
use serde_json::json;

use super::super::derive::{pass, start_draft, DraftJob};
use super::{rig, Rig};
use crate::testing::cx::REQUESTER_USER;

const PROMPT: &str = "fix what is broken";

fn plan_json() -> String {
    json!({"steps": [
        {"id": "audit", "title": "Audit", "kind": "work", "instructions": "list what is broken",
         "done_when": "listed", "evidence": ["broken.md"], "ok": "ship"},
        {"id": "ship", "title": "Ship", "kind": "deliver", "instructions": "open the PR",
         "done_when": "the PR exists"}]})
    .to_string()
}

impl Rig {
    /// A daily workflow whose plan moor was asked to make.
    fn wanting(&self) -> WorkflowId {
        self.answer_repo();
        let id = self.put(180);
        let store = self.cx.store().unwrap();
        store
            .set_plan_draft(id, Some(&PlanDraft::wanted(PROMPT)))
            .unwrap();
        id
    }

    /// One pass; the Box it asked for, if any.
    pub(super) fn plan_pass(&self) -> Option<DraftJob> {
        let job = RefCell::new(None);
        let out = pass(&self.cx, &|j| *job.borrow_mut() = Some(j));
        assert!(out.is_empty(), "{out:?}");
        job.into_inner()
    }

    pub(super) fn draft(&self, id: WorkflowId) -> Option<PlanDraft> {
        let drafts = self.cx.store().unwrap().plan_drafts().unwrap();
        drafts.into_iter().find(|(w, _)| *w == id).map(|(_, d)| d)
    }

    pub(super) fn open(&self, job: &DraftJob) -> Option<moor_core::BoxId> {
        let store = self.cx.store().unwrap();
        store.open_box_by_name(&job.req.name).unwrap().map(|r| r.id)
    }

    pub(super) fn leave_plan(&self, job: &DraftJob, text: &str) {
        let store = self.cx.store().unwrap();
        let id = self.open(job).expect("the Box is open");
        store
            .add_artifact(id, "plan.json", text.as_bytes(), self.cx.blobs().unwrap())
            .unwrap();
    }
}

#[test]
fn a_wanted_plan_starts_one_box_whose_plan_moor_stores_then_closes_it() {
    let r = rig();
    let id = r.wanting();
    let job = r.plan_pass().expect("a Box is asked for");
    assert!(is_draft_prompt(job.req.prompt.as_deref().unwrap()));
    assert!(job.req.prompt.as_deref().unwrap().contains(PROMPT));
    assert!(
        job.req.name.as_str().contains("-plan-"),
        "{}",
        job.req.name.as_str()
    );
    assert_eq!(
        r.draft(id).unwrap().box_name.as_deref(),
        Some(job.req.name.as_str())
    );
    start_draft(&r.cx, &job);
    assert!(r.open(&job).is_some());
    assert!(r.plan_pass().is_none(), "one Box per prompt");
    r.leave_plan(&job, &plan_json());
    assert!(r.plan_pass().is_none());
    let w = r.cx.store().unwrap().workflow_by_id(id).unwrap().unwrap();
    assert_eq!(w.plan.steps().count(), 2, "{:?}", w.plan);
    assert_eq!(
        w.plan.made_from,
        moor_core::workflow::PromptHash::of(PROMPT)
    );
    assert!(r.draft(id).is_none());
    assert!(
        r.open(&job).is_none(),
        "the Box is closed once its plan is stored"
    );
}

#[test]
fn a_plan_made_for_a_prompt_that_changed_since_is_never_stored_and_moor_asks_again() {
    let r = rig();
    let id = r.wanting();
    let job = r.plan_pass().unwrap();
    start_draft(&r.cx, &job);
    let mut def = WorkflowDef::new(
        moor_core::workflow::WorkflowName::parse("daily-fix").unwrap(),
        None,
        vec![super::repo()],
        "fix only the login page",
        (true, moor_core::workflow::TimeoutMinutes::DEFAULT),
    )
    .unwrap();
    def.plan = None;
    r.cx.store()
        .unwrap()
        .put_workflow(REQUESTER_USER, &def)
        .unwrap();
    r.leave_plan(&job, &plan_json());
    assert!(r.plan_pass().is_none());
    let w = r.cx.store().unwrap().workflow_by_id(id).unwrap().unwrap();
    assert!(
        w.plan.is_single_of(&w.prompt),
        "the old prompt's plan is not stored"
    );
    let draft = r.draft(id).unwrap();
    assert!(draft.is_for("fix only the login page") && draft.box_name.is_none());
    assert!(r.open(&job).is_none(), "the old prompt's Box is closed");
    assert!(
        r.plan_pass().is_some(),
        "the next pass asks for the new prompt's Box"
    );
}

#[test]
fn a_box_that_leaves_no_plan_leaves_the_reason_and_moor_does_not_try_again() {
    let r = rig();
    let id = r.wanting();
    let job = r.plan_pass().unwrap();
    start_draft(&r.cx, &job);
    let now = r.cx.clock().now().as_millis();
    r.advance_to(now + 31 * 60_000);
    assert!(r.plan_pass().is_none());
    let draft = r.draft(id).unwrap();
    assert!(
        draft
            .failed
            .as_deref()
            .unwrap()
            .contains("without leaving a plan"),
        "{draft:?}"
    );
    assert!(r.open(&job).is_none());
    assert!(r.plan_pass().is_none(), "no try repeats on its own");
}

#[test]
fn a_plan_that_is_not_valid_is_not_stored_and_says_why() {
    let r = rig();
    let id = r.wanting();
    let job = r.plan_pass().unwrap();
    start_draft(&r.cx, &job);
    r.leave_plan(
        &job,
        r#"{"steps": [{"id": "a", "title": "A", "kind": "work"}]}"#,
    );
    assert!(r.plan_pass().is_none());
    let draft = r.draft(id).unwrap();
    assert!(
        draft.failed.as_deref().unwrap().contains("not valid"),
        "{draft:?}"
    );
    let w = r.cx.store().unwrap().workflow_by_id(id).unwrap().unwrap();
    assert!(w.plan.is_single_of(&w.prompt));
}

#[test]
fn a_workflow_moor_reads_as_gates_gets_no_plan_box() {
    let r = rig();
    r.answer_repo();
    let gate = GateDoc {
        name: "proof".to_owned(),
        evidence: vec!["pr-*".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: None,
    };
    let mut def = WorkflowDef::new(
        moor_core::workflow::WorkflowName::parse("evidence").unwrap(),
        None,
        vec![super::repo()],
        PROMPT,
        (true, moor_core::workflow::TimeoutMinutes::DEFAULT),
    )
    .unwrap();
    def.gates = Some(Gates::parse(&[gate], &[]).unwrap());
    let store = r.cx.store().unwrap();
    let id = store.put_workflow(REQUESTER_USER, &def).unwrap();
    store
        .set_plan_draft(id, Some(&PlanDraft::wanted(PROMPT)))
        .unwrap();
    assert!(r.plan_pass().is_none());
    assert!(r.draft(id).is_none());
}
