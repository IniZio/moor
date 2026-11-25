//! moor reads a workflow's prompt into gates too: the Box's gates are stored as moor's reading,
//! clear the plan, and replace gates a person wrote only when the owner asked. The fixtures hold
//! the prompts of the live gated automations (client names swapped) and the gates a Box left for
//! them; each test asserts the checks those automations rely on.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "a test indexes what it just read and walks one scenario step by step"
)]

use moor_core::gate::{GateDoc, Gates, GatesDoc};
use moor_core::workflow::{PlanDraft, PromptHash, Workflow, WorkflowDef, WorkflowId};

use super::{rig, Rig};
use crate::testing::cx::REQUESTER_USER;

const PR_PROMPT: &str = include_str!("fixtures/pr-evidence.prompt.md");
const PR_CONFIG: &str = include_str!("fixtures/pr-evidence.config.yaml");
const PR_GATES: &str = include_str!("fixtures/pr-evidence.gates.json");
const DEPS_PROMPT: &str = include_str!("fixtures/deps-audit.prompt.md");
const DEPS_GATES: &str = include_str!("fixtures/deps-audit.gates.json");

impl Rig {
    /// A workflow of `prompt` with `gates` a person wrote, or none, whose reading moor was asked
    /// for; `replace` as the owner asked.
    fn reading(&self, prompt: &str, gates: Option<Gates>, replace: bool) -> WorkflowId {
        self.answer_repo();
        let mut def = WorkflowDef::new(
            moor_core::workflow::WorkflowName::parse("evidence").unwrap(),
            None,
            vec![super::repo()],
            prompt.trim_end(),
            (true, moor_core::workflow::TimeoutMinutes::DEFAULT),
        )
        .unwrap();
        def.config = Some(PR_CONFIG.to_owned());
        def.gates = gates;
        let store = self.cx.store().unwrap();
        let id = store.put_workflow(REQUESTER_USER, &def).unwrap();
        let wanted = PlanDraft::wanted(prompt.trim_end()).replacing(replace);
        store.set_plan_draft(id, Some(&wanted)).unwrap();
        id
    }

    /// Runs the Box moor asks for and has it leave `text` as its reading; the workflow after.
    fn read_into(&self, id: WorkflowId, text: &str) -> Workflow {
        let job = self.plan_pass().expect("a Box is asked for");
        super::super::derive::start_draft(&self.cx, &job);
        self.leave_plan(&job, text);
        assert!(self.plan_pass().is_none());
        assert!(
            self.open(&job).is_none(),
            "the Box is closed once its reading is stored"
        );
        assert!(self.draft(id).is_none());
        self.cx
            .store()
            .unwrap()
            .workflow_by_id(id)
            .unwrap()
            .unwrap()
    }
}

/// The gates `w` has, as written.
fn gates_of(w: &Workflow) -> GatesDoc {
    w.gates.as_ref().expect("gates are stored").doc()
}

/// The checks of gate `name`, as one text.
fn checks(doc: &GatesDoc, name: &str) -> String {
    let gate = doc.gates.iter().find(|g| g.name == name).expect(name);
    serde_json::to_string(&gate.checks).unwrap()
}

fn names(doc: &GatesDoc) -> Vec<&str> {
    doc.gates.iter().map(|g| g.name.as_str()).collect()
}

fn evidence<'a>(doc: &'a GatesDoc, name: &str) -> Vec<&'a str> {
    let gate = doc.gates.iter().find(|g| g.name == name).expect(name);
    gate.evidence.iter().map(String::as_str).collect()
}

#[test]
fn pr_evidence_gates_hold_the_proof_before_the_one_comment_is_checked() {
    let r = rig();
    let id = r.reading(PR_PROMPT, None, false);
    let job = r.plan_pass().unwrap();
    let asked = job.req.prompt.as_deref().unwrap();
    assert!(asked.contains("report_step for gate proof") && asked.contains("\"outcomes\""));
    assert!(
        asked.contains("PR_RERUN"),
        "the config's env is shown to the Box"
    );
    super::super::derive::start_draft(&r.cx, &job);
    r.leave_plan(&job, PR_GATES);
    assert!(r.plan_pass().is_none());
    let w = r.cx.store().unwrap().workflow_by_id(id).unwrap().unwrap();
    let doc = gates_of(&w);
    assert_eq!(names(&doc), ["proof", "done"], "proof passes before done");
    assert!(evidence(&doc, "proof").iter().any(|e| e.starts_with("pr-")));
    assert_eq!(evidence(&doc, "done"), ["posted.md"]);
    let done = checks(&doc, "done");
    assert!(
        done.contains("moor-evidence sha=") && done.contains("MOOR_PULL_HEAD_SHA"),
        "done checks the one evidence comment on the head: {done}"
    );
    assert!(doc
        .outcomes
        .iter()
        .any(|o| o.tone == Some(moor_core::gate::Tone::Bad)));
    assert!(
        doc.outcomes.iter().any(|o| o.anytime),
        "nothing to do can end a run early"
    );
    assert_eq!(
        w.gates.as_ref().unwrap().made_from(),
        Some(&PromptHash::of(PR_PROMPT.trim_end()))
    );
    assert!(
        w.plan.is_single_of(&w.prompt),
        "gates leave no plan to walk"
    );
}

#[test]
fn deps_audit_gates_check_review_audit_and_live_files_before_the_report() {
    let r = rig();
    let id = r.reading(DEPS_PROMPT, None, false);
    let w = r.read_into(id, DEPS_GATES);
    let doc = gates_of(&w);
    assert_eq!(names(&doc), ["review", "audit", "live", "report"]);
    assert_eq!(
        evidence(&doc, "review"),
        ["dependabot-review.md", "alerts.tsv"]
    );
    assert!(checks(&doc, "review").contains("dependabot/alerts"));
    assert_eq!(evidence(&doc, "audit"), ["audit.md", "upgrade-prs.txt"]);
    assert!(checks(&doc, "audit").contains("upgrade-prs.txt"));
    assert!(evidence(&doc, "live").contains(&"live.tsv"));
    assert!(checks(&doc, "live").contains("live.tsv"));
    assert_eq!(evidence(&doc, "report"), ["result.md"]);
    assert!(doc.outcomes.len() >= 2, "a run ends in a verdict");
}

#[test]
fn gates_a_person_wrote_are_replaced_only_when_the_owner_asked() {
    let by_hand = || {
        let gate = GateDoc {
            name: "proof".to_owned(),
            evidence: vec!["old.md".to_owned()],
            checks: Vec::new(),
            approval: None,
            max_tries: None,
        };
        Some(Gates::parse(&[gate], &[]).unwrap())
    };
    let kept = rig();
    let id = kept.reading(PR_PROMPT, by_hand(), false);
    assert!(kept.plan_pass().is_none(), "no Box reads it");
    let w = kept
        .cx
        .store()
        .unwrap()
        .workflow_by_id(id)
        .unwrap()
        .unwrap();
    assert_eq!(evidence(&gates_of(&w), "proof"), ["old.md"]);
    assert!(w.gates.as_ref().unwrap().made_from().is_none());

    let r = rig();
    let id = r.reading(PR_PROMPT, by_hand(), true);
    let w = r.read_into(id, PR_GATES);
    assert_eq!(names(&gates_of(&w)), ["proof", "done"]);
    assert!(w.gates.as_ref().unwrap().made_from().is_some());
}

#[test]
fn a_plan_moor_made_is_cleared_by_its_gates_and_gates_by_its_plan() {
    let r = rig();
    let id = r.reading(PR_PROMPT, None, false);
    let w = r.read_into(id, PR_GATES);
    assert!(w.gates.is_some() && w.plan.is_single_of(&w.prompt));
    r.cx.store()
        .unwrap()
        .set_plan_draft(id, Some(&PlanDraft::wanted(&w.prompt)))
        .unwrap();
    let plan = serde_json::json!({"steps": [
        {"id": "ship", "title": "Ship", "kind": "deliver", "instructions": "post it",
         "done_when": "posted"}]});
    let w = r.read_into(id, &plan.to_string());
    assert!(w.gates.is_none(), "a plan clears the gates: {:?}", w.gates);
    assert_eq!(w.plan.steps().count(), 1);
    assert!(!w.plan.is_single_of(&w.prompt));
}

#[test]
fn gates_that_are_not_valid_are_not_stored_and_say_why() {
    let r = rig();
    let id = r.reading(PR_PROMPT, None, false);
    let job = r.plan_pass().unwrap();
    super::super::derive::start_draft(&r.cx, &job);
    r.leave_plan(&job, r#"{"gates": [{"name": "proof"}]}"#);
    assert!(r.plan_pass().is_none());
    let why = r.draft(id).unwrap().failed.unwrap();
    assert!(why.contains("not valid") && why.contains("proof"), "{why}");
    let w = r.cx.store().unwrap().workflow_by_id(id).unwrap().unwrap();
    assert!(w.gates.is_none());
}
