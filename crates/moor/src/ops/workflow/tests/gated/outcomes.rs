//! A gated run with outcomes: they travel with the gates into its Box, and the run is delivered
//! in the outcome moor found, a bad one included.

use moor_core::gate::{CheckDoc, GateLine, Gates, OutcomeDoc, Tone};
use moor_core::workflow::RunOutcome;
use serde_json::json;

use super::super::FIRST_DUE;
use super::{proof, reported, rig};

fn outcome(name: &str, tone: Option<Tone>, anytime: bool) -> OutcomeDoc {
    OutcomeDoc {
        name: name.to_owned(),
        says: Some(format!("The pull request's {name}.")),
        tone,
        checks: vec![CheckDoc::from("true")],
        anytime,
    }
}

fn outcomes() -> Vec<OutcomeDoc> {
    vec![
        outcome("nothing to do", Some(Tone::Plain), true),
        outcome("check failed", Some(Tone::Bad), false),
        outcome("check passed", None, false),
    ]
}

fn held(stage: &str, name: &str) -> GateLine {
    serde_json::from_value(json!({"kind": "held", "gate": stage, "try_n": 1, "outcome": name}))
        .unwrap()
}

#[test]
fn a_run_whose_check_failed_is_delivered_in_the_bad_outcome_moor_found() {
    let r = rig();
    r.answer_repo();
    let id = r.put_gates(Gates::parse(&[proof()], &outcomes()).unwrap(), false);
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    let kept = r.cx.store().unwrap().box_gates(r.latest_box(id)).unwrap();
    assert_eq!(
        kept.unwrap().outcome_docs(),
        outcomes(),
        "frozen into the Box"
    );

    r.gate_line(id, &reported("proof", "pr 7 checked"));
    r.pass();
    assert_eq!(r.at(id), ("running", ("outcome".to_owned(), 1)));
    r.gate_line(id, &held("outcome", "check failed"));
    r.pass();
    let RunOutcome::Delivered { result, verdict } = r.outcome(id) else {
        panic!("{:?}", r.outcome(id));
    };
    let verdict = verdict.unwrap();
    assert_eq!(
        (result.as_str(), verdict.name.as_str(), verdict.tone),
        ("pr 7 checked", "check failed", Tone::Bad)
    );
    assert!(r.told(id).contains(&"outcome_reached".to_owned()));
}

#[test]
fn an_anytime_outcome_delivers_the_run_from_its_first_gate() {
    let r = rig();
    r.answer_repo();
    let id = r.put_gates(Gates::parse(&[proof()], &outcomes()).unwrap(), false);
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    r.gate_line(id, &held("proof", "nothing to do"));
    r.pass();
    let RunOutcome::Delivered { verdict, .. } = r.outcome(id) else {
        panic!("{:?}", r.outcome(id));
    };
    assert_eq!(verdict.unwrap().name.as_str(), "nothing to do");
    let told = r.told(id);
    assert!(
        !told.iter().any(|t| t.starts_with("gate_passed")),
        "{told:?}"
    );
}
