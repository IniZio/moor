//! `workflow-add` with a plan and an expected revision, and `workflow-list` showing both.

use moor_core::request::WorkflowAddReq;
use moor_core::workflow::PromptHash;
use serde_json::{json, Value};

use super::{add, add_req, cmd, me, ok, refusal};
use crate::ops::users::tests::fixture;
use moor_core::request::WorkflowCmd;

fn step(id: &str, ok: &str) -> Value {
    json!({"id": id, "title": id, "kind": "work", "instructions": "do it",
           "done_when": "done", "evidence": ["out.md"], "ok": ok})
}

fn ship() -> Value {
    json!({"id": "ship", "title": "Ship", "kind": "deliver",
           "instructions": "open the PR", "done_when": "the PR exists"})
}

fn planned(steps: &[Value]) -> WorkflowAddReq {
    let mut req = add_req("daily", "0 9 * * *", "UTC");
    req.plan = Some(json!({ "steps": steps }).to_string());
    req
}

#[test]
fn a_stored_plan_is_listed_and_the_reply_shows_it_as_numbered_steps() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let added = ok(
        &f,
        &who,
        add(planned(&[step("audit", "ship"), ship()]), None),
    );
    assert_eq!(added["revision"], 0);
    let lines = added["plan"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].as_str().unwrap().starts_with("1. audit (work)"));
    assert!(lines[1].as_str().unwrap().starts_with("2. ship (deliver)"));

    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    let w = &listed["workflows"][0];
    assert_eq!(w["revision"], 0);
    assert_eq!(w["plan_stale"], false);
    assert_eq!(w["plan"]["steps"][0]["id"], "audit");
    assert_eq!(w["plan"]["steps"][1]["kind"], "deliver");
    assert_eq!(
        w["plan"]["made_from"],
        PromptHash::of("fix what is broken").hex()
    );
}

#[test]
fn a_workflow_without_a_plan_lists_none_and_is_not_stale() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let added = ok(&f, &who, add(add_req("daily", "0 9 * * *", "UTC"), None));
    assert!(added.get("plan").is_none());
    let w = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert!(w.get("plan").is_none());
    assert_eq!(w["plan_stale"], false);
}

#[test]
fn a_bad_plan_is_refused_with_every_error_and_nothing_is_saved() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let bad = planned(&[
        json!({"id": "A", "title": "", "kind": "work"}),
        json!({"id": "b", "title": "B", "kind": "juggle"}),
    ]);
    let why = refusal(&f, &who, add(bad, None));
    assert!(why.contains("errors, so nothing was saved"), "{why}");
    assert!(why.contains("invalid step id"), "{why}");
    assert!(why.contains("unknown kind `juggle`"), "{why}");
    assert!(why.contains("needs a deliver step"), "{why}");
    assert!(why.matches("\n- ").count() >= 5, "{why}");
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    assert_eq!(listed["workflows"], json!([]));
}

#[test]
fn a_plan_that_is_not_json_is_refused() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = add_req("daily", "0 9 * * *", "UTC");
    req.plan = Some("not json".to_owned());
    let why = refusal(&f, &who, add(req, None));
    assert!(why.contains("the plan is not valid"), "{why}");
}

#[test]
fn check_and_approve_steps_are_refused_until_moor_runs_them() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let req = planned(&[
        json!({"id": "gate", "title": "Gate", "kind": "check", "command": "true", "ok": "ship"}),
        json!({"id": "ask", "title": "Ask", "kind": "approve", "question": "ok?", "ok": "ship"}),
        ship(),
    ]);
    let why = refusal(&f, &who, add(req, None));
    assert!(why.contains("step `gate` is a check step"), "{why}");
    assert!(why.contains("step `ask` is a approve step"), "{why}");
    assert_eq!(ok(&f, &who, cmd(WorkflowCmd::List))["workflows"], json!([]));
}

#[test]
fn every_write_bumps_the_revision_and_a_stale_one_is_refused() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let first = ok(&f, &who, add(planned(&[ship()]), None));
    assert_eq!(first["revision"], 0);

    let mut again = planned(&[step("audit", "ship"), ship()]);
    again.expected_revision = Some(0);
    assert_eq!(ok(&f, &who, add(again.clone(), None))["revision"], 1);

    let why = refusal(&f, &who, add(again.clone(), None));
    assert!(
        why.contains("is at revision 1, not revision 0")
            && why.contains("changed after you read it"),
        "{why}"
    );
    let w = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert_eq!(w["revision"], 1);
    assert_eq!(w["plan"]["steps"].as_array().unwrap().len(), 2);

    again.expected_revision = Some(1);
    assert_eq!(ok(&f, &who, add(again, None))["revision"], 2);
}

#[test]
fn an_expected_revision_for_a_workflow_that_does_not_exist_is_refused() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = planned(&[ship()]);
    req.expected_revision = Some(0);
    let why = refusal(&f, &who, add(req, None));
    assert!(why.contains("does not exist"), "{why}");
}

#[test]
fn a_plan_made_for_an_older_prompt_is_stale_and_replacing_without_a_plan_clears_it() {
    let f = fixture();
    let who = me(&f, "local:2000");
    ok(&f, &who, add(planned(&[ship()]), None));

    // The author changes the prompt and sends back the plan as `list_workflows` showed it.
    let shown = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0]["plan"].clone();
    let mut edited = add_req("daily", "0 9 * * *", "UTC");
    edited.prompt = "a different prompt".to_owned();
    edited.plan = Some(shown.to_string());
    ok(&f, &who, add(edited.clone(), None));
    let w = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert_eq!(w["plan_stale"], true);
    assert!(w.get("plan").is_some(), "a stale plan is kept and shown");

    edited.plan = None;
    ok(&f, &who, add(edited, None));
    let w = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert!(w.get("plan").is_none());
    assert_eq!(w["plan_stale"], false);
}
