//! A workflow carries gates in place of a plan: saved, listed, kept when a replace leaves them
//! out, cleared by an empty list, and never saved beside a plan.

use moor_core::gate::{Approval, GateDoc, Gates};
use moor_core::request::WorkflowAddReq;
use moor_core::request::WorkflowCmd;
use serde_json::json;

use super::{add, add_req, cmd, me, ok, refusal, stored};
use crate::ops::users::tests::fixture;

fn proof() -> GateDoc {
    GateDoc {
        name: "proof".to_owned(),
        evidence: vec!["pr-*".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: Some(3),
    }
}

fn done() -> GateDoc {
    GateDoc {
        name: "done".to_owned(),
        evidence: Vec::new(),
        checks: vec!["true".into()],
        approval: Some(Approval::Owner),
        max_tries: None,
    }
}

fn gated(gates: Vec<GateDoc>) -> WorkflowAddReq {
    let mut req = add_req("daily", "0 9 * * *", "UTC");
    req.gates = Some(gates);
    req
}

fn plan_text() -> String {
    json!({"steps": [{"id": "ship", "title": "Ship", "kind": "deliver",
                      "instructions": "open the PR", "done_when": "the PR exists"}]})
    .to_string()
}

#[test]
fn gates_with_a_plan_are_refused_and_nothing_is_saved() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = gated(vec![proof()]);
    req.plan = Some(plan_text());
    let why = refusal(&f, &who, add(req, None));
    assert!(why.contains("a plan or gates, not both"), "{why}");
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    assert_eq!(listed["workflows"], json!([]), "nothing was stored");
}

#[test]
fn gates_alone_are_saved_and_read_back() {
    let f = fixture();
    let who = me(&f, "local:2000");
    ok(&f, &who, add(gated(vec![proof(), done()]), None));
    let got = stored(&f, &who);
    assert_eq!(
        got.gates.as_ref().map(Gates::docs),
        Some(vec![proof(), done()])
    );
    assert!(got.plan.is_single_of(&got.prompt), "gates are no plan");
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    let w = &listed["workflows"][0];
    assert_eq!(w["gates"][0]["name"], "proof");
    assert_eq!(w["gates"][0]["evidence"], json!(["pr-*"]));
    assert_eq!(w["gates"][1]["approval"], "owner");
    assert!(w.get("plan").is_none(), "{w}");
}

#[test]
fn a_replace_that_leaves_gates_out_keeps_them_and_an_empty_list_clears_them() {
    let f = fixture();
    let who = me(&f, "local:2000");
    ok(&f, &who, add(gated(vec![proof()]), None));
    let mut again = add_req("daily", "0 9 * * *", "UTC");
    again.prompt = "edited".to_owned();
    ok(&f, &who, add(again, None));
    let got = stored(&f, &who);
    assert_eq!(got.prompt, "edited");
    assert_eq!(got.gates.as_ref().map(Gates::docs), Some(vec![proof()]));

    let mut clear = add_req("daily", "0 9 * * *", "UTC");
    clear.gates = Some(Vec::new());
    ok(&f, &who, add(clear, None));
    assert_eq!(stored(&f, &who).gates, None, "an empty list clears them");
}

#[test]
fn a_plan_sent_to_a_workflow_that_keeps_its_gates_is_refused_with_the_way_out() {
    let f = fixture();
    let who = me(&f, "local:2000");
    ok(&f, &who, add(gated(vec![proof()]), None));
    let mut planned = add_req("daily", "0 9 * * *", "UTC");
    planned.plan = Some(plan_text());
    let why = refusal(&f, &who, add(planned.clone(), None));
    assert!(why.contains("gates: []"), "{why}");
    assert_eq!(
        stored(&f, &who).gates.as_ref().map(Gates::docs),
        Some(vec![proof()])
    );

    planned.gates = Some(Vec::new());
    ok(&f, &who, add(planned, None));
    let got = stored(&f, &who);
    assert_eq!(
        (got.gates, got.plan.is_single_of(&got.prompt)),
        (None, false)
    );
}

#[test]
fn bad_gates_are_refused_naming_the_gate() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut bare = proof();
    bare.evidence = Vec::new();
    let why = refusal(&f, &who, add(gated(vec![bare]), None));
    assert!(why.contains("gate `proof` needs evidence"), "{why}");
}

fn outcomes() -> Vec<moor_core::gate::OutcomeDoc> {
    serde_json::from_value(json!([
        {"name": "nothing to do", "tone": "plain", "checks": ["test -z \"$MOOR_PULL_NUMBER\""], "anytime": true},
        {"name": "check failed", "tone": "bad", "checks": ["false"]},
        {"name": "check passed"}
    ]))
    .unwrap()
}

#[test]
fn outcomes_are_saved_with_their_gates_and_listed_beside_them() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = gated(vec![proof()]);
    req.outcomes = Some(Box::new(outcomes()));
    ok(&f, &who, add(req, None));
    let got = stored(&f, &who);
    assert_eq!(
        got.gates.as_ref().map(Gates::outcome_docs),
        Some(outcomes())
    );
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    let w = &listed["workflows"][0];
    assert_eq!(w["outcomes"][1]["name"], "check failed");
    assert_eq!(w["outcomes"][1]["tone"], "bad");
}

#[test]
fn outcomes_without_gates_or_that_could_skip_them_are_refused() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = add_req("daily", "0 9 * * *", "UTC");
    req.outcomes = Some(Box::new(outcomes()));
    let why = refusal(&f, &who, add(req, None));
    assert!(why.contains("outcomes with the gates"), "{why}");
    let mut req = gated(vec![proof()]);
    req.outcomes = Some(Box::new(
        serde_json::from_value(json!([{"name": "skip", "anytime": true}])).unwrap(),
    ));
    let why = refusal(&f, &who, add(req, None));
    assert!(why.contains("anytime and needs a check"), "{why}");
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    assert_eq!(listed["workflows"], json!([]), "nothing was stored");
}

#[test]
fn gates_sent_without_outcomes_keep_the_stored_outcomes_and_an_empty_list_clears_them() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = gated(vec![proof()]);
    req.outcomes = Some(Box::new(outcomes()));
    ok(&f, &who, add(req, None));
    ok(&f, &who, add(gated(vec![proof(), done()]), None));
    let got = stored(&f, &who);
    assert_eq!(
        got.gates.as_ref().map(Gates::docs),
        Some(vec![proof(), done()])
    );
    assert_eq!(
        got.gates.as_ref().map(Gates::outcome_docs),
        Some(outcomes()),
        "outcomes left out are the stored ones"
    );
    let mut clear = gated(vec![proof()]);
    clear.outcomes = Some(Box::default());
    ok(&f, &who, add(clear, None));
    assert_eq!(
        stored(&f, &who).gates.as_ref().map(Gates::outcome_docs),
        Some(Vec::new()),
        "an empty list leaves the implicit done"
    );
}
