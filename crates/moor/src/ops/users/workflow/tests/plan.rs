//! `workflow-add` keeps moor's reading of a workflow, its plan and gates, which no request sends;
//! and the expected revision.

use moor_core::gate::{GateDoc, Gates};
use moor_core::name::ForgeRepo;
use moor_core::request::{Request, WorkflowCmd};
use moor_core::user::Identity;
use moor_core::workflow::{Plan, TimeoutMinutes, WorkflowDef, WorkflowName, WorkflowSchedule};
use serde_json::json;

use super::{add, add_req, cmd, me, ok, refusal};
use crate::ops::users::tests::{fixture, Fixture};

const PROMPT: &str = "fix what is broken";

/// The plan moor would have made for `PROMPT`: an audit, then a pull request.
fn derived() -> Plan {
    let text = json!({"steps": [
        {"id": "audit", "title": "Audit", "kind": "work", "instructions": "do it",
         "done_when": "done", "evidence": ["out.md"], "ok": "ship"},
        {"id": "ship", "title": "Ship", "kind": "deliver",
         "instructions": "open the PR", "done_when": "the PR exists"}]});
    Plan::parse(&text.to_string(), PROMPT).unwrap()
}

/// Stores `who`'s workflow `daily` as moor left it: with `plan` or `gates`.
fn seed(f: &Fixture, who: &Identity, plan: Option<Plan>, gates: Option<Gates>) {
    let store = f.cx.store().unwrap();
    let user = store.user_of(who).unwrap().unwrap();
    let mut def = WorkflowDef::new(
        WorkflowName::parse("daily").unwrap(),
        Some(WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap()),
        vec![ForgeRepo::new("o", "n")],
        PROMPT,
        (true, TimeoutMinutes::DEFAULT),
    )
    .unwrap();
    def.plan = plan;
    def.gates = gates;
    store.put_workflow(user, &def).unwrap();
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
fn every_write_bumps_the_revision_and_a_stale_one_is_refused() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let first = ok(&f, &who, add(add_req("daily", "0 9 * * *", "UTC"), None));
    assert_eq!(first["revision"], 0);

    let mut again = add_req("daily", "0 9 * * *", "UTC");
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

    again.expected_revision = Some(1);
    assert_eq!(ok(&f, &who, add(again, None))["revision"], 2);
}

#[test]
fn an_expected_revision_for_a_workflow_that_does_not_exist_is_refused() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = add_req("daily", "0 9 * * *", "UTC");
    req.expected_revision = Some(0);
    let why = refusal(&f, &who, add(req, None));
    assert!(why.contains("does not exist"), "{why}");
}

#[test]
fn a_replace_keeps_moors_plan_and_a_changed_prompt_marks_it_stale() {
    let f = fixture();
    let who = me(&f, "local:2000");
    seed(&f, &who, Some(derived()), None);

    let mut cron = add_req("daily", "0 10 * * *", "UTC");
    cron.prompt = PROMPT.to_owned();
    let added = ok(&f, &who, add(cron, None));
    assert_eq!(added["plan"].as_array().map(Vec::len), Some(2), "{added}");
    let w = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert_eq!(w["plan"]["steps"][0]["id"], "audit");
    assert_eq!(w["plan_stale"], false);

    let mut reworded = add_req("daily", "0 10 * * *", "UTC");
    reworded.prompt = "fix only the login page".to_owned();
    ok(&f, &who, add(reworded, None));
    let w = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert_eq!(
        w["plan"]["steps"][0]["id"], "audit",
        "a stale plan is kept and shown"
    );
    assert_eq!(w["plan_stale"], true);
}

#[test]
fn a_replace_keeps_the_gates_moor_has() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let proof = GateDoc {
        name: "proof".to_owned(),
        evidence: vec!["pr-*".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: None,
    };
    seed(&f, &who, None, Some(Gates::parse(&[proof], &[]).unwrap()));
    let mut req = add_req("daily", "0 9 * * *", "UTC");
    req.prompt = "collect the evidence".to_owned();
    ok(&f, &who, add(req, None));
    let w = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert_eq!(w["gates"][0]["name"], "proof", "{w}");
    assert!(w.get("plan").is_none());
}

#[test]
fn a_request_that_sends_a_plan_gates_or_outcomes_is_refused() {
    for field in ["plan", "gates", "outcomes"] {
        let mut req = json!({"cmd": "workflow-add", "name": "daily", "repos": ["o/n"],
                             "prompt": PROMPT});
        req[field] = json!([]);
        let parsed = serde_json::from_value::<Request>(req);
        let why = parsed.expect_err(field).to_string();
        assert!(why.contains(field), "{why}");
    }
}

/// A fixture whose controller lets Box agents report steps, so a planned run can go ahead.
fn reporting() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let served = moor_core::config::parse_user("x", "moor_service:\n  upstream: 127.0.0.1:8444\n");
    let config = moor_core::config::UserConfig {
        moor_service: served.unwrap().moor_service,
        ..Default::default()
    };
    let cx = crate::cx::Cx::fake_with(
        dir.path(),
        std::sync::Arc::new(crate::testing::driver::FakeDriver::new()),
        std::sync::Arc::new(crate::testing::proc::ScriptedProc::new()),
        config,
    )
    .with_env(|e| e.with_front_step_reports(std::collections::BTreeSet::from(["gate".to_owned()])));
    let vault = crate::vault::Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    Fixture {
        _dir: dir,
        cx,
        vault,
    }
}

fn making(f: &Fixture, who: &Identity) -> serde_json::Value {
    ok(f, who, cmd(WorkflowCmd::List))["workflows"][0]["plan_making"].clone()
}

#[test]
fn a_new_workflow_or_a_new_prompt_asks_moor_for_a_plan_and_an_unchanged_one_does_not() {
    let f = reporting();
    let who = me(&f, "local:2000");
    ok(&f, &who, add(add_req("daily", "0 9 * * *", "UTC"), None));
    let asked = making(&f, &who);
    assert_eq!(
        asked["prompt"],
        moor_core::workflow::PromptHash::of(PROMPT).hex()
    );
    assert!(asked.get("box_name").is_none(), "{asked}");

    let store = f.cx.store().unwrap();
    let user = store.user_of(&who).unwrap().unwrap();
    let id = store
        .workflow(user, &WorkflowName::parse("daily").unwrap())
        .unwrap()
        .unwrap()
        .id;
    store.set_plan_draft(id, None).unwrap();
    ok(&f, &who, add(add_req("daily", "0 10 * * *", "UTC"), None));
    assert!(making(&f, &who).is_null(), "a cron change asks for nothing");

    let mut reworded = add_req("daily", "0 10 * * *", "UTC");
    reworded.prompt = "fix only the login page".to_owned();
    ok(&f, &who, add(reworded, None));
    assert_eq!(
        making(&f, &who)["prompt"],
        moor_core::workflow::PromptHash::of("fix only the login page").hex()
    );
}

#[test]
fn regenerate_plan_asks_again_for_the_same_prompt_and_replaces_gates_a_person_wrote() {
    let f = reporting();
    let who = me(&f, "local:2000");
    seed(&f, &who, Some(derived()), None);
    let mut again = add_req("daily", "0 9 * * *", "UTC");
    again.regenerate_plan = true;
    ok(&f, &who, add(again.clone(), None));
    assert_eq!(
        making(&f, &who)["prompt"],
        moor_core::workflow::PromptHash::of(PROMPT).hex()
    );

    let g = reporting();
    let who = me(&g, "local:2000");
    seed(&g, &who, None, Some(Gates::parse(&[proof()], &[]).unwrap()));
    let mut reworded = add_req("daily", "0 9 * * *", "UTC");
    reworded.prompt = "collect it".to_owned();
    ok(&g, &who, add(reworded, None));
    assert!(
        making(&g, &who).is_null(),
        "gates a person wrote are kept through a prompt change"
    );
    let w = ok(&g, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert_eq!(w["gates_by"], "person", "{w}");
    again.prompt = "collect it".to_owned();
    ok(&g, &who, add(again, None));
    let (_, draft) = g.cx.store().unwrap().plan_drafts().unwrap().remove(0);
    assert!(draft.is_for("collect it") && draft.replace, "{draft:?}");
    let w = ok(&g, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert_eq!(
        w["gates"][0]["name"], "proof",
        "kept until moor's reading is stored"
    );
}

fn proof() -> GateDoc {
    GateDoc {
        name: "proof".to_owned(),
        evidence: vec!["pr-*".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: None,
    }
}

#[test]
fn gates_moor_made_go_stale_with_the_prompt_and_moor_reads_it_again() {
    let f = reporting();
    let who = me(&f, "local:2000");
    let made = Gates::parse(&[proof()], &[]).unwrap().read_from(PROMPT);
    seed(&f, &who, None, Some(made));
    let w = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert!(w["gates_by"] == "moor" && w["plan_stale"] == false, "{w}");
    let mut reworded = add_req("daily", "0 9 * * *", "UTC");
    reworded.prompt = "collect it".to_owned();
    ok(&f, &who, add(reworded, None));
    let w = ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0].clone();
    assert_eq!(
        w["plan_stale"], true,
        "moor's gates of an earlier prompt are stale"
    );
    assert_eq!(
        making(&f, &who)["prompt"],
        moor_core::workflow::PromptHash::of("collect it").hex(),
        "and moor reads the new prompt"
    );
}

#[test]
fn a_controller_whose_boxes_cannot_report_steps_asks_for_no_plan() {
    let f = fixture();
    let who = me(&f, "local:2000");
    ok(&f, &who, add(add_req("daily", "0 9 * * *", "UTC"), None));
    assert!(
        making(&f, &who).is_null(),
        "its runs deliver the prompt in one step"
    );
}

#[test]
fn a_new_prompt_while_a_box_makes_the_plan_keeps_that_box_on_the_draft() {
    let f = reporting();
    let who = me(&f, "local:2000");
    ok(&f, &who, add(add_req("daily", "0 9 * * *", "UTC"), None));
    let store = f.cx.store().unwrap();
    let user = store.user_of(&who).unwrap().unwrap();
    let id = store
        .workflow(user, &WorkflowName::parse("daily").unwrap())
        .unwrap()
        .unwrap()
        .id;
    let (_, wanted) = store.plan_drafts().unwrap().remove(0);
    let asked = wanted.asked("daily/w1-plan-x", 1);
    assert!(store.move_plan_draft(id, &wanted, Some(&asked)).unwrap());
    let mut reworded = add_req("daily", "0 9 * * *", "UTC");
    reworded.prompt = "fix only the login page".to_owned();
    reworded.regenerate_plan = true;
    ok(&f, &who, add(reworded, None));
    assert_eq!(
        making(&f, &who)["box_name"],
        "daily/w1-plan-x",
        "the pass still knows the Box, so it closes it before asking again"
    );
}
