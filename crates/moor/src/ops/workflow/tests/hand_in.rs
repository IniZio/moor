//! The Box reading an automation's prompt hands in phrases and effects; moor checks them, keeps
//! the reading with its gates, and closes the Box. Three refusals end the reading.
#![allow(
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "a test indexes what it just read and walks one scenario step by step"
)]

use moor_core::reading::{Effect, Origin};
use moor_core::request::{ReadingHandInReq, ReadingHandInValue, UserCmd, WorkflowCmd};
use moor_core::workflow::{
    PlanDraft, TimeoutMinutes, Workflow, WorkflowDef, WorkflowId, WorkflowName,
};
use moor_core::BoxId;
use serde_json::{json, Value};

use super::super::derive::{start_draft, DraftJob};
use super::super::reading_hand_in;
use super::{rig, Rig};
use crate::error::CommandError;
use crate::ops::workflow::now::run_with;
use crate::ops::workflow::run_start;
use crate::testing::cx::{requester, REQUESTER_USER};
use crate::vault::Vault;

const PROMPT: &str = include_str!("fixtures/pr-evidence.prompt.md");
const CONFIG: &str = include_str!("fixtures/pr-evidence.config.yaml");
const GATES: &str = include_str!("fixtures/pr-evidence.gates.json");
const COMMENT: &str = "POST /repos/{repo}/issues/*/comments";

/// A hand-in that holds the one comment by its intent and the done gate by its report.
fn whole() -> Value {
    json!({
        "phrases": [
            {"quote": {"exact": "ONE comment on that pull request"},
             "intents": [{"intent": "comment", "polarity": "do"}]},
            {"quote": {"exact": "call report_step for gate done"}, "holds": ["done"]},
            {"quote": {"exact": "Never push a branch"}}
        ],
        "effects": [
            {"kind": "gate", "label": "done", "gate": {"name": "done", "evidence": ["posted.md"]}}
        ]
    })
}

/// A hand-in that holds only the one comment, by its intent.
fn comment_only() -> Value {
    json!({
        "phrases": [{"quote": {"exact": "ONE comment on that pull request"},
                     "intents": [{"intent": "comment", "polarity": "do"}]}],
        "effects": []
    })
}

impl Rig {
    /// The evidence automation, with the Box moor started to read its prompt.
    fn reader(&self) -> (WorkflowId, DraftJob, BoxId) {
        self.answer_repo();
        let mut def = WorkflowDef::new(
            WorkflowName::parse("evidence").unwrap(),
            None,
            vec![super::repo()],
            PROMPT.trim_end(),
            (true, TimeoutMinutes::DEFAULT),
        )
        .unwrap();
        def.config = Some(CONFIG.to_owned());
        let store = self.cx.store().unwrap();
        let id = store.put_workflow(REQUESTER_USER, &def).unwrap();
        store
            .set_plan_draft(id, Some(&PlanDraft::wanted(PROMPT.trim_end())))
            .unwrap();
        let job = self.plan_pass().expect("a Box is asked for");
        start_draft(&self.cx, &job);
        let box_id = self.open(&job).expect("the reader's Box is open");
        (id, job, box_id)
    }

    fn hand_in(&self, box_id: BoxId, doc: &Value) -> Result<ReadingHandInValue, CommandError> {
        let req = ReadingHandInReq {
            box_id,
            reading: serde_json::from_value(doc.clone()).unwrap(),
        };
        reading_hand_in(&self.cx, &req)
    }

    fn workflow(&self, id: WorkflowId) -> Workflow {
        self.cx
            .store()
            .unwrap()
            .workflow_by_id(id)
            .unwrap()
            .unwrap()
    }
}

fn gate_names(w: &Workflow) -> Vec<String> {
    let doc = w.gates.as_ref().expect("gates are stored").doc();
    doc.gates.into_iter().map(|g| g.name).collect()
}

#[test]
fn the_third_refused_hand_in_fails_the_draft_naming_the_quote() {
    let r = rig();
    let (id, job, box_id) = r.reader();
    let mut doc = whole();
    doc["phrases"][2]["quote"]["exact"] = json!("Never fly to the moon");
    for n in 1..=2 {
        let why = r.hand_in(box_id, &doc).unwrap_err().error.to_string();
        assert!(why.contains("Never fly to the moon"), "{why}");
        assert_eq!(r.draft(id).unwrap().refused, n);
        assert!(
            r.open(&job).is_some(),
            "the reader may fix it and hand in again"
        );
    }
    let why = r.hand_in(box_id, &doc).unwrap_err().error.to_string();
    assert!(why.contains("Never fly to the moon"), "{why}");
    let failed = r.draft(id).unwrap().failed.expect("the draft failed");
    assert!(failed.contains("Never fly to the moon"), "{failed}");
    assert!(r.open(&job).is_none(), "the reader's Box is closed");
    let w = r.workflow(id);
    assert!(w.gates.is_none());
    let store = r.cx.store().unwrap();
    assert_eq!(store.workflow_reading(REQUESTER_USER, id).unwrap(), None);
}

#[test]
fn a_hand_in_from_a_box_that_is_not_the_reader_is_refused() {
    let r = rig();
    let (id, _, _) = r.reader();
    let daily = r.put(180);
    let start = |job| {
        run_start(&r.cx, &job);
        Ok(())
    };
    let read = |_: &_| unreachable!("no pull request");
    run_with(
        &r.cx,
        r.cx.store().unwrap(),
        REQUESTER_USER,
        ("daily-fix", None, None),
        (&read, &start),
    )
    .unwrap();
    let run_box = r.latest_box(daily);
    let why = r.hand_in(run_box, &whole()).unwrap_err().error.to_string();
    assert!(why.contains("not reading"), "{why}");
    let why = r
        .hand_in(BoxId::new(9_999), &whole())
        .unwrap_err()
        .error
        .to_string();
    assert!(why.contains("9999"), "{why}");
    let draft = r.draft(id).unwrap();
    assert_eq!(draft.refused, 0, "another Box's call counts for no reader");
    assert!(draft.failed.is_none());
}

#[test]
fn an_accepted_hand_in_stores_the_reading_with_its_gates_clears_the_draft_and_closes_the_box() {
    let r = rig();
    let (id, job, box_id) = r.reader();
    let reply = r.hand_in(box_id, &whole()).unwrap().reply;
    assert!(reply.contains("accepted"), "{reply}");
    assert!(r.draft(id).is_none());
    assert!(r.open(&job).is_none(), "the reader's Box is closed");
    let w = r.workflow(id);
    assert_eq!(gate_names(&w), ["done"]);
    assert!(w.gates.as_ref().unwrap().made_from().is_some());
    assert!(
        w.plan.is_single_of(&w.prompt),
        "gates leave no plan to walk"
    );
    let store = r.cx.store().unwrap();
    let held = store.workflow_reading(REQUESTER_USER, id).unwrap().unwrap();
    let reading = store.reading(REQUESTER_USER, &held).unwrap().unwrap();
    assert_eq!(reading.source, w.prompt);
    assert_eq!(reading.context.repos, w.repos);
    let moor_core::reading::ReadingState::Read(read) = &reading.state else {
        panic!("{:?}", reading.state)
    };
    let quoted: Vec<&str> = read
        .phrases()
        .iter()
        .map(|p| &reading.source[p.at.start()..p.at.end()])
        .collect();
    assert_eq!(
        quoted,
        [
            "Never push a branch",
            "ONE comment on that pull request",
            "call report_step for gate done"
        ]
    );
    assert_eq!(read.effects().len(), 3);
    assert_eq!(
        read.reader().map(|by| by.box_name.as_str()),
        Some(job.req.name.as_str())
    );
}

#[test]
fn the_comment_intent_holds_its_row_lines_less_the_read_the_rules_allow() {
    let r = rig();
    let (id, _, box_id) = r.reader();
    r.hand_in(box_id, &whole()).unwrap();
    let store = r.cx.store().unwrap();
    let held = store.workflow_reading(REQUESTER_USER, id).unwrap().unwrap();
    let reading = store.reading(REQUESTER_USER, &held).unwrap().unwrap();
    assert_eq!(reading.context.table.map(|t| t.0), Some(1));
    let moor_core::reading::ReadingState::Read(read) = &reading.state else {
        panic!("{:?}", reading.state)
    };
    let lines: Vec<_> = read
        .effects()
        .iter()
        .filter_map(|e| match e {
            Effect::Allow {
                rule, entry, from, ..
            } => Some((rule.as_str(), entry.as_str(), *from)),
            _ => None,
        })
        .collect();
    assert_eq!(
        lines,
        [
            ("github-api", COMMENT, Origin::Table),
            (
                "github-api",
                "PATCH /repos/{repo}/issues/comments/*",
                Origin::Table
            ),
        ]
    );
}

#[test]
fn a_repo_the_automation_lacks_is_refused_by_name() {
    let r = rig();
    let (id, _, box_id) = r.reader();
    let mut doc = whole();
    doc["phrases"][0]["intents"][0]["repo"] = json!("acme/elsewhere");
    let why = r.hand_in(box_id, &doc).unwrap_err().error.to_string();
    assert!(why.contains("acme/elsewhere"), "{why}");
    assert_eq!(r.draft(id).unwrap().refused, 1);
    assert!(r.workflow(id).gates.is_none());
}

#[test]
fn a_gates_plan_file_still_lands_as_a_reading_with_no_phrases() {
    let r = rig();
    let (id, job, _) = r.reader();
    r.leave_plan(&job, GATES);
    assert!(r.plan_pass().is_none());
    assert!(r.draft(id).is_none());
    assert!(r.open(&job).is_none());
    let w = r.workflow(id);
    assert_eq!(gate_names(&w), ["proof", "done"]);
    let store = r.cx.store().unwrap();
    let held = store.workflow_reading(REQUESTER_USER, id).unwrap().unwrap();
    let reading = store.reading(REQUESTER_USER, &held).unwrap().unwrap();
    let moor_core::reading::ReadingState::Read(read) = &reading.state else {
        panic!("{:?}", reading.state)
    };
    assert!(read.phrases().is_empty());
    assert!(read.effects().len() >= 2, "{:?}", read.effects());
}

#[test]
fn a_reading_with_no_gate_leaves_the_gates_the_workflow_had() {
    let r = rig();
    let (id, _, box_id) = r.reader();
    r.hand_in(box_id, &whole()).unwrap();
    let store = r.cx.store().unwrap();
    let first = store.workflow_reading(REQUESTER_USER, id).unwrap();
    store
        .set_plan_draft(id, Some(&PlanDraft::wanted(PROMPT.trim_end())))
        .unwrap();
    r.advance_to(60_000);
    let job = r.plan_pass().expect("a Box is asked for");
    start_draft(&r.cx, &job);
    let box_id = r.open(&job).unwrap();
    r.hand_in(box_id, &comment_only()).unwrap();
    assert_eq!(gate_names(&r.workflow(id)), ["done"]);
    let now = store.workflow_reading(REQUESTER_USER, id).unwrap();
    assert!(now.is_some() && now != first, "the new reading is held");
}

/// The reading the person sees on `id`, as the workflow list sends it.
fn listed_reading(r: &Rig, id: WorkflowId) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let list = UserCmd::Workflow(WorkflowCmd::List);
    let v = crate::ops::users::run(&r.cx, &vault, &requester(), &list).unwrap();
    let name = r.workflow(id).name.to_string();
    let w = v["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == name.as_str());
    w.expect("the workflow is listed")["reading"].clone()
}

#[test]
fn an_accepted_reading_is_listed_as_current() {
    let r = rig();
    let (id, _, box_id) = r.reader();
    r.hand_in(box_id, &whole()).unwrap();
    let shown = listed_reading(&r, id);
    assert_eq!(shown["state"], "read");
    assert_eq!(shown["stale"], json!([]), "{shown}");
}

/// A step plan a reader may leave in place of a reading.
const STEPS: &str = r#"{"steps": [{"id": "ship", "title": "Ship", "kind": "deliver",
    "instructions": "post the comment", "done_when": "it is posted"}]}"#;

impl Rig {
    /// Asks for `id`'s prompt to be read again; the new reader's job and Box.
    fn read_again(&self, id: WorkflowId) -> (DraftJob, BoxId) {
        let store = self.cx.store().unwrap();
        store
            .set_plan_draft(id, Some(&PlanDraft::wanted(PROMPT.trim_end())))
            .unwrap();
        self.advance_to(60_000);
        let job = self.plan_pass().expect("a Box is asked for");
        start_draft(&self.cx, &job);
        let box_id = self.open(&job).expect("the reader's Box is open");
        (job, box_id)
    }
}

#[test]
fn a_reading_after_a_step_plan_leaves_no_step_plan_to_walk() {
    let r = rig();
    let (id, job, _) = r.reader();
    r.leave_plan(&job, STEPS);
    assert!(r.plan_pass().is_none());
    assert!(!r.workflow(id).plan.is_single_of(PROMPT.trim_end()));
    let (_, box_id) = r.read_again(id);
    r.hand_in(box_id, &comment_only()).unwrap();
    let w = r.workflow(id);
    assert!(
        w.plan.is_single_of(&w.prompt),
        "the reading replaced the step plan"
    );
}

#[test]
fn a_step_plan_after_a_reading_leaves_no_reading_held() {
    let r = rig();
    let (id, _, box_id) = r.reader();
    r.hand_in(box_id, &whole()).unwrap();
    let (job, _) = r.read_again(id);
    r.leave_plan(&job, STEPS);
    assert!(r.plan_pass().is_none());
    assert!(!r.workflow(id).plan.is_single_of(PROMPT.trim_end()));
    let shown = listed_reading(&r, id);
    assert!(
        shown.is_null(),
        "a step-plan automation holds no reading: {shown}"
    );
}
