//! Tests for [`Plan::parse`]: every rule, and every error at once.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "test assertions index JSON values and panic on a wrong shape"
)]

use serde_json::{json, Value};

use super::{Plan, PlanError, PromptHash, StepKind};

const PROMPT: &str = "keep the dependencies current";

fn work(id: &str, ok: &str) -> Value {
    json!({
        "id": id, "title": "Work", "kind": "work",
        "instructions": "do it", "done_when": "it is done",
        "evidence": ["report.md"], "ok": ok
    })
}

fn deliver(id: &str) -> Value {
    json!({
        "id": id, "title": "Deliver", "kind": "deliver",
        "instructions": "open the pull request", "done_when": "the PR exists"
    })
}

fn parse(steps: &[Value]) -> Result<Plan, Vec<PlanError>> {
    Plan::parse(&json!({ "steps": steps }).to_string(), PROMPT)
}

fn errors(steps: &[Value]) -> Vec<PlanError> {
    parse(steps).expect_err("the plan should be refused")
}

#[test]
fn a_plan_of_every_kind_parses() {
    let plan = parse(&[
        json!({"id": "audit", "title": "Audit", "kind": "check", "command": "npm audit", "ok": "fix", "fail": "fix"}),
        json!({"id": "fix", "title": "Fix", "kind": "work", "max_tries": 3,
               "instructions": "update", "done_when": "tests pass",
               "evidence": ["before-*", "after.txt"], "ok": "ask", "fail": "audit"}),
        json!({"id": "ask", "title": "Ask", "kind": "approve", "question": "ship it?", "ok": "ship"}),
        deliver("ship"),
    ])
    .unwrap();
    assert_eq!(plan.step_count(), 4);
    assert_eq!(plan.first().id.as_str(), "audit");
    assert_eq!(plan.made_from, PromptHash::of(PROMPT));
    assert!(!plan.edited);
    let fix = plan.steps().nth(1).unwrap();
    assert_eq!(fix.max_tries.get(), 3);
    assert_eq!(fix.kind.name(), "work");
    let StepKind::Work { evidence, next, .. } = &fix.kind else {
        panic!("not a work step");
    };
    assert_eq!(evidence.names().count(), 2);
    assert_eq!(next.fail.as_ref().unwrap().as_str(), "audit");
}

#[test]
fn a_plan_round_trips_through_its_stored_json() {
    let plan = parse(&[work("a", "b"), deliver("b")]).unwrap();
    let again = Plan::parse(&plan.to_json().unwrap(), "some other prompt").unwrap();
    assert_eq!(again, plan);
    assert_eq!(again.made_from, PromptHash::of(PROMPT));
}

#[test]
fn a_loop_needs_no_rule_if_a_deliver_step_is_reachable() {
    let steps = [
        json!({"id": "a", "title": "A", "kind": "work", "instructions": "x", "done_when": "y",
               "evidence": ["e"], "ok": "a", "fail": "b"}),
        deliver("b"),
    ];
    assert!(parse(&steps).is_ok());
}

#[test]
fn step_count_is_one_to_twenty() {
    assert_eq!(errors(&[]), vec![PlanError::NoSteps]);
    let mut many: Vec<Value> = (0..20).map(|i| work(&format!("s{i}"), "end")).collect();
    many.push(deliver("end"));
    assert_eq!(errors(&many), vec![PlanError::TooManySteps(21)]);
    many.remove(0);
    assert!(parse(&many).is_ok());
}

#[test]
fn ids_are_bounded_and_unique() {
    for bad in ["", "1a", "A", "a_b", "a b", &"a".repeat(33)] {
        let errs = errors(&[json!({
            "id": bad, "title": "t", "kind": "deliver", "instructions": "i", "done_when": "d"
        })]);
        assert!(
            matches!(errs.as_slice(), [PlanError::BadId { .. }]),
            "{bad}: {errs:?}"
        );
    }
    assert!(parse(&[deliver(&"a".repeat(32))]).is_ok());
    assert_eq!(
        errors(&[work("a", "b"), deliver("b"), deliver("b")]),
        vec![PlanError::DuplicateId("b".to_owned())]
    );
}

#[test]
fn titles_and_tries_are_bounded() {
    let mut long = deliver("a");
    long["title"] = json!("t".repeat(121));
    assert!(matches!(
        errors(&[long]).as_slice(),
        [PlanError::Title { len: 121, .. }]
    ));
    let mut empty = deliver("a");
    empty["title"] = json!("  ");
    assert!(matches!(
        errors(&[empty]).as_slice(),
        [PlanError::Title { .. }]
    ));
    for bad in [0_u64, 6, 300] {
        let mut s = work("a", "b");
        s["max_tries"] = json!(bad);
        let errs = errors(&[s, deliver("b")]);
        assert!(matches!(errs.as_slice(), [PlanError::MaxTries { got, .. }] if *got == bad));
    }
    for ok in [1, 5] {
        let mut s = work("a", "b");
        s["max_tries"] = json!(ok);
        assert!(parse(&[s, deliver("b")]).is_ok());
    }
}

#[test]
fn every_edge_names_a_step() {
    let mut a = work("a", "gone");
    a["fail"] = json!("nowhere");
    let errs = errors(&[a, deliver("b")]);
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs
        .iter()
        .all(|e| matches!(e, PlanError::UnknownTarget { .. })));
}

#[test]
fn some_deliver_step_is_reachable_from_every_step() {
    assert_eq!(
        errors(&[work("a", "b"), work("b", "a")]),
        vec![PlanError::NoDeliver]
    );
    let pocket = [
        work("a", "b"),
        deliver("b"),
        work("loop1", "loop2"),
        work("loop2", "loop1"),
    ];
    let errs = errors(&pocket);
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs.iter().all(|e| matches!(e, PlanError::Trapped { .. })));
}

#[test]
fn a_deliver_step_has_no_onward_edge() {
    let mut s = deliver("a");
    s["ok"] = json!("a");
    s["fail"] = json!("a");
    let errs = errors(&[s]);
    assert_eq!(errs.len(), 2);
    assert!(errs.iter().all(|e| matches!(
        e,
        PlanError::Unexpected {
            kind: "deliver",
            ..
        }
    )));
}

#[test]
fn a_work_step_needs_evidence_and_an_ok_edge() {
    let mut s = work("a", "b");
    s.as_object_mut().unwrap().remove("evidence");
    s.as_object_mut().unwrap().remove("ok");
    let errs = errors(&[s, deliver("b")]);
    assert!(errs.contains(&PlanError::Missing {
        step: "step `a`".to_owned(),
        kind: "work",
        field: "evidence"
    }));
    assert!(errs
        .iter()
        .any(|e| matches!(e, PlanError::Missing { field: "ok", .. })));
    let mut empty = work("a", "b");
    empty["evidence"] = json!([]);
    assert!(matches!(
        errors(&[empty, deliver("b")]).as_slice(),
        [PlanError::Missing {
            field: "evidence",
            ..
        }]
    ));
}

#[test]
fn evidence_names_are_flat_files_or_prefixes() {
    for ok in ["report.md", "before-*", "a", "A.b_c-d*"] {
        let mut s = work("a", "b");
        s["evidence"] = json!([ok]);
        assert!(parse(&[s, deliver("b")]).is_ok(), "{ok}");
    }
    for bad in ["", "*", "dir/x", "../x", ".hidden", "a*b", "a**", "a b"] {
        let mut s = work("a", "b");
        s["evidence"] = json!([bad]);
        let errs = errors(&[s, deliver("b")]);
        assert!(
            matches!(errs.as_slice(), [PlanError::BadEvidence { .. }]),
            "{bad}: {errs:?}"
        );
    }
}

#[test]
fn a_check_has_a_command_and_no_report_fields() {
    let mut s = json!({"id": "c", "title": "C", "kind": "check", "ok": "d",
                       "instructions": "x", "evidence": ["e"]});
    let errs = errors(&[s.clone(), deliver("d")]);
    assert!(errs.iter().any(|e| matches!(
        e,
        PlanError::Missing {
            field: "command",
            ..
        }
    )));
    assert!(errs.iter().any(|e| matches!(
        e,
        PlanError::Unexpected {
            field: "instructions",
            ..
        }
    )));
    assert!(errs.iter().any(|e| matches!(
        e,
        PlanError::Unexpected {
            field: "evidence",
            ..
        }
    )));
    s["command"] = json!("true");
    s.as_object_mut().unwrap().remove("instructions");
    s.as_object_mut().unwrap().remove("evidence");
    assert!(parse(&[s, deliver("d")]).is_ok());
}

#[test]
fn an_approve_has_a_question() {
    let s = json!({"id": "q", "title": "Q", "kind": "approve", "ok": "d"});
    assert!(matches!(
        errors(&[s, deliver("d")]).as_slice(),
        [PlanError::Missing {
            field: "question",
            ..
        }]
    ));
}

#[test]
fn text_is_at_most_four_kib() {
    let mut s = work("a", "b");
    s["instructions"] = json!("x".repeat(4097));
    s["done_when"] = json!("y".repeat(4097));
    let errs = errors(&[s, deliver("b")]);
    assert_eq!(errs.len(), 2);
    assert!(errs
        .iter()
        .all(|e| matches!(e, PlanError::TooLong { len: 4097, .. })));
    let mut cmd = json!({"id": "c", "title": "C", "kind": "check", "ok": "d"});
    cmd["command"] = json!("z".repeat(4097));
    assert!(matches!(
        errors(&[cmd.clone(), deliver("d")]).as_slice(),
        [PlanError::TooLong {
            field: "command",
            ..
        }]
    ));
    cmd["command"] = json!("z".repeat(4096));
    assert!(parse(&[cmd, deliver("d")]).is_ok());
}

#[test]
fn an_unknown_kind_is_named() {
    let s = json!({"id": "a", "title": "A", "kind": "think"});
    assert_eq!(
        errors(&[s, deliver("d")]),
        vec![PlanError::UnknownKind {
            step: "step `a`".to_owned(),
            kind: "think".to_owned()
        }]
    );
}

#[test]
fn every_error_comes_back_at_once() {
    let steps = [
        json!({"id": "A", "title": "", "kind": "work", "max_tries": 9}),
        json!({"id": "b", "title": "B", "kind": "check", "ok": "nowhere"}),
        json!({"id": "b", "title": "B", "kind": "approve", "ok": "b"}),
        json!({"id": "d", "title": "D", "kind": "deliver", "ok": "b"}),
        json!({"id": "e", "title": "E", "kind": "juggle"}),
    ];
    let errs = errors(&steps);
    let text: Vec<String> = errs.iter().map(ToString::to_string).collect();
    // bad id, title, tries, the four work fields (instructions, done_when, evidence, ok), the
    // check's command and unknown target, a duplicate id, the approve's question, the deliver's
    // edge and instructions and done-when, and an unknown kind.
    assert_eq!(errs.len(), 15, "{text:#?}");
}

#[test]
fn text_that_is_not_a_plan_is_one_error() {
    for bad in [
        "",
        "not json",
        "[]",
        r#"{"steps": 3}"#,
        r#"{"steps": [], "extra": 1}"#,
    ] {
        let errs = Plan::parse(bad, PROMPT).unwrap_err();
        assert!(
            matches!(errs.as_slice(), [PlanError::Json(_)]),
            "{bad}: {errs:?}"
        );
    }
}

#[test]
fn made_from_marks_a_plan_stale_and_comes_from_the_prompt_when_absent() {
    let plan = parse(&[deliver("a")]).unwrap();
    assert_eq!(plan.made_from, PromptHash::of(PROMPT));
    let old = PromptHash::of("an older prompt").hex();
    let text = json!({"steps": [deliver("a")], "made_from": old}).to_string();
    let kept = Plan::parse(&text, PROMPT).unwrap();
    assert_eq!(kept.made_from, PromptHash::of("an older prompt"));
    assert_ne!(kept.made_from, PromptHash::of(PROMPT));
    let bad = json!({"steps": [deliver("a")], "made_from": "xyz"}).to_string();
    assert_eq!(
        Plan::parse(&bad, PROMPT).unwrap_err(),
        vec![PlanError::MadeFrom]
    );
}

#[test]
fn the_single_plan_is_one_deliver_step() {
    let plan = Plan::single(PROMPT);
    assert_eq!(plan.step_count(), 1);
    assert!(plan.is_single_of(PROMPT));
    assert!(!plan.is_single_of("another"));
    assert!(plan.first().kind.next().is_none());
}
