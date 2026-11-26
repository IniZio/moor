//! What a workflow's list shows of its reading: the held one judged against now, stored gates
//! read as effects, nothing for a step plan, and nothing of another person's.

use moor_core::gate::{GateDoc, Gates};
use moor_core::name::{BoxName, ForgeRepo};
use moor_core::reading::{
    Context, Read, ReaderRun, Reading, ReadingId, ReadingOf, ReadingState, RulesPin,
};
use moor_core::request::{RepoFile, WorkflowCmd};
use moor_core::seam::clock::Ts;
use moor_core::user::Identity;
use moor_core::workflow::{
    Plan, PromptHash, TimeoutMinutes, WorkflowDef, WorkflowId, WorkflowName, WorkflowSchedule,
    READER_INSTRUCTIONS,
};
use serde_json::{json, Value};

use super::{add, add_req, cmd, me, ok};
use crate::ops::users::tests::{fixture, Fixture};

const PROMPT: &str = "Take screenshots of each page. Post one comment on the pull request.";
const COMMENT: &str = "POST /repos/{repo}/issues/*/comments";
const CONFIG: &str =
    "proxy:\n  allow:\n    github-api:\n      - \"POST /repos/{repo}/issues/*/comments\"\n";

/// Stores `who`'s workflow `daily` of PROMPT with a step `plan`, `gates` and `config`.
fn seed(
    f: &Fixture,
    who: &Identity,
    (plan, gates): (Option<Plan>, Option<Gates>),
    config: Option<&str>,
) -> WorkflowId {
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
    def.config = config.map(str::to_owned);
    store.put_workflow(user, &def).unwrap()
}

/// A reading of `of` that moor read, pinned to the rules text of revision `rules` of o/n.
fn read_reading(of: WorkflowId, who: &Identity, f: &Fixture, rules: u64) -> Reading {
    let user = f.cx.store().unwrap().user_of(who).unwrap().unwrap();
    let reading = Reading {
        id: ReadingId::parse("01936a3e-7c2b-7d4e-8f00-0123456789ab").unwrap(),
        of: ReadingOf::Automation(of),
        parent: None,
        source: PROMPT.into(),
        context: Context {
            source_hash: PromptHash::of(PROMPT),
            repos: vec![ForgeRepo::new("o", "n")],
            rules: vec![RulesPin {
                repo: Some(ForgeRepo::new("o", "n")),
                revision: rules,
            }],
            instructions: READER_INSTRUCTIONS,
            table: None,
        },
        made_by: user,
        state: ReadingState::Reading {
            by: ReaderRun {
                box_name: BoxName::parse("o/reader-1").unwrap(),
                harness: "claude".into(),
                model: None,
            },
            since: Ts::from_millis(1),
        },
    };
    // Slice 1 kept this read from a hand-in whose allow equalled the config's proxy.allow line.
    let at = |exact: &str| {
        let start = PROMPT.find(exact).unwrap();
        json!({"start": start, "end": start + exact.len()})
    };
    let read: Read = serde_json::from_value(json!({
        "phrases": [
            {"id": "p1", "quote": {"exact": "Take screenshots of each page"}, "at": at("Take screenshots of each page"),
             "mark": {"kind": "held", "effects": ["e1"]}},
            {"id": "p2", "quote": {"exact": "Post one comment on the pull request"}, "at": at("Post one comment on the pull request"),
             "mark": {"kind": "held", "effects": ["e2"]}}
        ],
        "effects": [
            {"kind": "gate", "id": "e1", "doc": {"name": "screenshots", "evidence": ["shot-*"]}},
            {"kind": "allow", "id": "e2", "rule": "github-api", "entry": COMMENT}
        ],
        "reader": {"box_name": "o/reader-1", "harness": "claude"}
    }))
    .unwrap();
    Reading {
        state: ReadingState::Read(read),
        ..reading
    }
}

/// `who`'s workflow `daily` holding a reading pinned to the rules of revision 1 of o/n.
fn held(f: &Fixture, who: &Identity) -> Reading {
    let store = f.cx.store().unwrap();
    store
        .set_repo_text(
            &ForgeRepo::new("o", "n"),
            RepoFile::Rules,
            "rules: {}",
            "operator",
            None,
        )
        .unwrap();
    let id = seed(f, who, (None, None), Some(CONFIG));
    let reading = read_reading(id, who, f, 1);
    store.hold_read(&reading).unwrap();
    reading
}

fn listed(f: &Fixture, who: &Identity) -> Vec<Value> {
    let v = ok(f, who, cmd(WorkflowCmd::List));
    v["workflows"].as_array().unwrap().clone()
}

fn gates() -> Gates {
    let proof = GateDoc {
        name: "proof".to_owned(),
        evidence: vec!["pr-*".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: None,
    };
    Gates::parse(&[proof], &[]).unwrap()
}

#[test]
fn stored_gates_with_no_reading_are_listed_as_effects_and_no_phrases() {
    let f = fixture();
    let who = me(&f, "local:2000");
    seed(&f, &who, (None, Some(gates())), None);

    let r = listed(&f, &who)[0]["reading"].clone();

    assert_eq!(r["id"], Value::Null);
    assert_eq!(r["phrases"], json!([]));
    assert_eq!(r["effects"][0]["kind"], "gate");
    assert_eq!(r["effects"][0]["line"], "proof");
    assert_eq!(r["source"], PROMPT);
}

#[test]
fn a_held_reading_is_listed_with_its_phrases_and_the_lines_they_light() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let reading = held(&f, &who);

    let r = listed(&f, &who)[0]["reading"].clone();

    assert_eq!(r["id"], reading.id.as_str());
    assert_eq!(r["stale"], json!([]));
    assert_eq!(
        r["phrases"][1],
        json!({"id": "p2", "mark": "held", "effects": ["e2"]})
    );
    assert_eq!(r["effects"][1]["line"], COMMENT);
    assert_eq!(r["may"], json!({"github-api": [COMMENT]}));
    assert_eq!(r["gates"]["gates"][0]["name"], "screenshots");
}

#[test]
fn a_workflow_with_a_step_plan_has_no_reading() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let steps = json!({"steps": [{"id": "audit", "title": "Audit", "kind": "deliver",
                                  "instructions": "do it", "done_when": "done"}]});
    seed(
        &f,
        &who,
        (Some(Plan::parse(&steps.to_string(), PROMPT).unwrap()), None),
        None,
    );

    let w = listed(&f, &who)[0].clone();

    assert!(w.get("reading").is_none(), "{w}");
    assert_eq!(w["plan"]["steps"][0]["id"], "audit");
}

#[test]
fn stale_lists_prompt_after_the_prompt_changes() {
    let f = fixture();
    let who = me(&f, "local:2000");
    held(&f, &who);

    let mut edited = add_req("daily", "0 9 * * *", "UTC");
    edited.repos = vec!["o/n".to_owned()];
    edited.config = Some(CONFIG.to_owned());
    edited.prompt = format!("{PROMPT} Then stop.");
    ok(&f, &who, add(edited, None));

    let r = listed(&f, &who)[0]["reading"].clone();
    assert_eq!(r["stale"], json!(["prompt"]));
    assert_eq!(r["source"], PROMPT, "the reading keeps the words it read");
}

#[test]
fn stale_lists_rules_after_a_rules_save() {
    let f = fixture();
    let who = me(&f, "local:2000");
    held(&f, &who);

    f.cx.store()
        .unwrap()
        .set_repo_text(
            &ForgeRepo::new("o", "n"),
            RepoFile::Rules,
            "rules: {}\n",
            "operator",
            Some(1),
        )
        .unwrap();

    let r = listed(&f, &who)[0]["reading"].clone();
    assert_eq!(r["stale"], json!(["rules"]));
}

#[test]
fn a_config_that_drops_an_allow_line_leaves_its_phrase_asked_and_the_file_without_it() {
    let f = fixture();
    let who = me(&f, "local:2000");
    held(&f, &who);

    let mut bare = add_req("daily", "0 9 * * *", "UTC");
    bare.repos = vec!["o/n".to_owned()];
    bare.prompt = PROMPT.to_owned();
    bare.config = Some(String::new());
    ok(&f, &who, add(bare, None));

    let r = listed(&f, &who)[0]["reading"].clone();
    assert_eq!(r["phrases"][1]["mark"], "asked");
    assert_eq!(r["phrases"][0]["mark"], "held");
    assert_eq!(r["may"], json!({}));
    assert_eq!(r["effects"].as_array().map(Vec::len), Some(1));
}

#[test]
fn another_users_workflow_reading_is_not_returned() {
    let f = fixture();
    let alice = me(&f, "local:2000");
    let bob = me(&f, "local:2001");
    let reading = held(&f, &alice);
    seed(&f, &bob, (None, None), None);

    let theirs = listed(&f, &bob);
    assert!(theirs[0].get("reading").is_none(), "{}", theirs[0]);
    let store = f.cx.store().unwrap();
    let bob_id = store.user_of(&bob).unwrap().unwrap();
    assert!(store.reading(bob_id, &reading.id).unwrap().is_none());
    assert!(!listed(&f, &alice)[0]["reading"].is_null());
}
