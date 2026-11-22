//! A check's plain sentence: the two wire forms, what is refused, and where it is read first.

#![allow(
    clippy::indexing_slicing,
    reason = "a test indexes the JSON it just built"
)]

use serde_json::json;

use super::*;
use crate::workflow::StepId;

const SAYS: &str = "The file never.txt exists.";

fn said(run: &str, says: &str) -> CheckDoc {
    CheckDoc::Said(SaidDoc {
        run: run.to_owned(),
        says: says.to_owned(),
    })
}

fn gates(evidence: &[&str], checks: Vec<CheckDoc>) -> Gates {
    Gates::parse(
        &[GateDoc {
            name: "done".to_owned(),
            evidence: evidence.iter().map(|s| (*s).to_owned()).collect(),
            checks,
            approval: None,
            max_tries: Some(1),
        }],
        &[],
    )
    .unwrap()
}

fn failed(check: u8) -> GateLine {
    GateLine::Checked {
        gate: StepId::parse("done").unwrap(),
        try_n: 1,
        check,
        exit: 1,
        tail: "no never.txt\n".to_owned(),
    }
}

#[test]
fn a_check_is_a_bare_command_or_a_command_with_its_sentence() {
    let text = json!({"gates": [{"name": "done", "checks": ["true", {"run": "test -f never.txt", "says": SAYS}]}]});
    let parsed = Gates::from_json(&text.to_string()).unwrap();
    let checks = &parsed.get(0).unwrap().checks;
    assert_eq!(
        (checks[0].run.as_str(), checks[0].says.as_deref()),
        ("true", None)
    );
    assert_eq!(
        (checks[1].run.as_str(), checks[1].says.as_deref()),
        ("test -f never.txt", Some(SAYS))
    );
    let back = serde_json::to_value(parsed.doc()).unwrap();
    assert_eq!(
        back, text,
        "a bare command stays bare and a sentence stays beside its command"
    );
}

fn refused(c: CheckDoc) -> GateError {
    Gates::parse(&[gate_with(c)], &[]).unwrap_err()
}

#[test]
fn a_blank_or_long_or_multi_line_sentence_is_refused() {
    let got = refused(said("true", "   "));
    assert!(matches!(got, GateError::EmptySays { .. }), "{got:?}");
    let got = refused(said("true", "one\ntwo"));
    assert!(matches!(got, GateError::SaysNotOneLine { .. }), "{got:?}");
    let got = refused(said("true", &"x".repeat(SAYS_MAX + 1)));
    assert!(
        matches!(got, GateError::SaysTooLong { len: 201, .. }),
        "{got:?}"
    );
    assert!(Gates::parse(&[gate_with(said("true", &"x".repeat(SAYS_MAX)))], &[]).is_ok());
    let got = refused(said(" ", SAYS));
    assert!(matches!(got, GateError::EmptyCheck { .. }), "{got:?}");
}

#[test]
fn an_unknown_field_or_a_missing_command_is_refused() {
    let text =
        json!({"gates": [{"name": "done", "checks": [{"run": "true", "says": SAYS, "extra": 1}]}]});
    assert!(matches!(
        Gates::from_json(&text.to_string()),
        Err(GateError::Json(_))
    ));
    let text = json!({"gates": [{"name": "done", "checks": [{"says": SAYS}]}]});
    assert!(Gates::from_json(&text.to_string()).is_err());
}

fn gate_with(check: CheckDoc) -> GateDoc {
    GateDoc {
        name: "done".to_owned(),
        evidence: Vec::new(),
        checks: vec![check],
        approval: None,
        max_tries: None,
    }
}

#[test]
fn the_reason_and_the_steer_lead_with_the_sentence_and_keep_the_command() {
    let g = gates(&[], vec![said("test -f never.txt", SAYS)]);
    let why = match status(&g, &[failed(0)]) {
        GateStatus::Stuck { why, .. } => why,
        other => format!("{other:?}"),
    };
    assert_eq!(why, format!("out of tries: check 1 (“{SAYS}”) exited 1"));
    let gate = g.get(0).unwrap();
    let steer = gate.failed_check_steer(0, ("exited 1", "no never.txt\n"));
    assert!(
        steer.starts_with(&format!(
            "Gate `done` is not passed: check 1, “{SAYS}” (`test -f never.txt`), exited 1."
        )),
        "{steer}"
    );
    let plain = gates(&[], vec!["test -f never.txt".into()]);
    let steer = plain
        .get(0)
        .unwrap()
        .failed_check_steer(0, ("exited 1", ""));
    assert!(
        steer.starts_with("Gate `done` is not passed: check 1 (`test -f never.txt`) exited 1."),
        "{steer}"
    );
}

#[test]
fn the_agent_is_told_what_moor_looks_for() {
    let g = gates(
        &["posted.md"],
        vec![said("a", "A holds."), "b".into(), said("c", "C holds.")],
    );
    let brief = g.get(0).unwrap().brief();
    assert!(
        brief.contains("moor runs its 3 checks itself once you are quiet. It looks for: “A holds.” “C holds.”."),
        "{brief}"
    );
    let bare = gates(&[], vec!["b".into()]).get(0).unwrap().brief();
    assert!(!bare.contains("looks for"), "{bare}");
}

#[test]
fn a_stuck_wait_carries_the_sentence_beside_the_command_and_a_bare_one_has_none() {
    let g = gates(&[], vec![said("test -f never.txt", SAYS)]);
    let w = wait(&g, &[failed(0)]).unwrap();
    assert_eq!(w.says.as_deref(), Some(SAYS));
    assert_eq!(w.command.as_deref(), Some("test -f never.txt"));
    let json = serde_json::to_value(&w).unwrap();
    assert_eq!(json["says"], SAYS);
    let bare = wait(&gates(&[], vec!["false".into()]), &[failed(0)]).unwrap();
    assert!(serde_json::to_value(&bare).unwrap().get("says").is_none());
}

#[test]
fn a_run_page_step_lists_each_check_with_its_sentence() {
    let g = gates(
        &["posted.md"],
        vec![said("test -f never.txt", SAYS), "true".into()],
    );
    let doc = g.steps_doc();
    let step = &doc["steps"][0];
    assert_eq!(
        step["checks"],
        json!([{"run": "test -f never.txt", "says": SAYS}, {"run": "true", "says": null}])
    );
    assert_eq!(step["command"], "test -f never.txt; true");
}
