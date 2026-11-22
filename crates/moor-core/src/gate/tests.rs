//! Gates as written: what parses, what is refused, and the wire forms.

use super::*;
use crate::workflow::{Answer, StepReport, StepResult};

fn gate(name: &str) -> GateDoc {
    GateDoc {
        name: name.to_owned(),
        evidence: vec!["proof.md".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: None,
    }
}

fn refused(docs: &[GateDoc]) -> GateError {
    Gates::parse(docs, &[]).unwrap_err()
}

#[test]
fn one_gate_of_each_kind_parses() {
    let docs = [
        gate("proof"),
        GateDoc {
            evidence: Vec::new(),
            checks: vec!["cargo test".into()],
            ..gate("tests")
        },
        GateDoc {
            evidence: Vec::new(),
            approval: Some(Approval::Owner),
            max_tries: Some(3),
            ..gate("owner-ok")
        },
    ];
    let gates = Gates::parse(&docs, &[]).unwrap();
    let names: Vec<_> = gates.iter().map(|g| g.name.to_string()).collect();
    assert_eq!(names, ["proof", "tests", "owner-ok"]);
    let last = gates.get(2).unwrap();
    assert_eq!(
        (last.approval, last.max_tries.get(), last.evidence.is_none()),
        (Approval::Owner, 3, true)
    );
    assert_eq!(gates.get(0).unwrap().max_tries, Tries::TWO);
}

#[test]
fn a_gate_with_no_max_tries_gets_two_and_an_explicit_one_stays_one() {
    let one = GateDoc {
        max_tries: Some(1),
        ..gate("once")
    };
    let gates = Gates::parse(&[gate("twice"), one], &[]).unwrap();
    let tries: Vec<_> = gates.iter().map(|g| g.max_tries.get()).collect();
    assert_eq!(tries, [2, 1]);
    let docs = gates.docs();
    assert_eq!(
        docs.iter().map(|d| d.max_tries).collect::<Vec<_>>(),
        [None, Some(1)]
    );
    assert_eq!(Gates::parse(&docs, &[]).unwrap(), gates);
}

#[test]
fn no_gates_are_refused() {
    assert_eq!(refused(&[]), GateError::NoGates);
}

#[test]
fn more_than_eight_gates_are_refused() {
    let nine: Vec<_> = (0..9).map(|i| gate(&format!("g{i}"))).collect();
    assert_eq!(refused(&nine), GateError::TooManyGates(9));
    assert!(Gates::parse(nine.get(..8).unwrap(), &[]).is_ok());
}

#[test]
fn a_name_used_twice_is_refused() {
    assert_eq!(
        refused(&[gate("proof"), gate("tests"), gate("proof")]),
        GateError::DuplicateName("proof".to_owned())
    );
}

#[test]
fn a_gate_with_no_evidence_no_checks_and_no_approval_is_refused() {
    let empty = GateDoc {
        evidence: Vec::new(),
        ..gate("nothing")
    };
    assert_eq!(
        refused(&[empty]),
        GateError::NothingToPass {
            gate: "nothing".to_owned()
        }
    );
    let owner = GateDoc {
        evidence: Vec::new(),
        approval: Some(Approval::Owner),
        ..gate("nothing")
    };
    assert!(Gates::parse(&[owner], &[]).is_ok());
}

fn evidence_gate(names: &[&str]) -> GateDoc {
    GateDoc {
        evidence: names.iter().map(|s| (*s).to_owned()).collect(),
        ..gate("proof")
    }
}

fn check_gate(commands: Vec<CheckDoc>) -> GateDoc {
    GateDoc {
        checks: commands,
        ..gate("proof")
    }
}

#[test]
fn a_bad_name_is_refused() {
    let got = refused(&[gate("Not A Name")]);
    assert!(matches!(got, GateError::BadName { .. }), "{got:?}");
}

#[test]
fn evidence_must_be_flat_names_or_prefixes_and_few() {
    let got = refused(&[evidence_gate(&["a/b"])]);
    assert!(matches!(got, GateError::BadEvidence { .. }), "{got:?}");
    assert!(Gates::parse(&[evidence_gate(&["before-*", "after.png"])], &[]).is_ok());
    let many: Vec<String> = (0..33).map(|i| format!("f{i}")).collect();
    let names: Vec<&str> = many.iter().map(String::as_str).collect();
    let got = refused(&[evidence_gate(&names)]);
    assert!(matches!(got, GateError::TooManyEvidence { .. }), "{got:?}");
}

#[test]
fn checks_must_be_few_filled_and_short() {
    let blank = refused(&[check_gate(vec!["  ".into()])]);
    assert!(matches!(blank, GateError::EmptyCheck { .. }), "{blank:?}");
    let long = refused(&[check_gate(vec!["x".repeat(4097).into()])]);
    assert!(matches!(long, GateError::CheckTooLong { .. }), "{long:?}");
    let many = refused(&[check_gate(vec!["true".into(); 9])]);
    assert!(matches!(many, GateError::TooManyChecks { .. }), "{many:?}");
    assert!(Gates::parse(&[check_gate(vec!["true".into(); 8])], &[]).is_ok());
}

#[test]
fn tries_must_be_one_to_five() {
    for tries in [0, 6] {
        let got = refused(&[GateDoc {
            max_tries: Some(tries),
            ..gate("proof")
        }]);
        assert!(matches!(got, GateError::MaxTries { .. }), "{got:?}");
    }
    for tries in [1, 5] {
        let ok = GateDoc {
            max_tries: Some(tries),
            ..gate("proof")
        };
        assert!(Gates::parse(&[ok], &[]).is_ok());
    }
}

#[test]
fn gates_survive_their_json_and_leave_out_what_is_default() {
    let docs = vec![
        gate("proof"),
        GateDoc {
            evidence: Vec::new(),
            checks: vec!["make lint".into()],
            approval: Some(Approval::Owner),
            max_tries: Some(3),
            ..gate("tests")
        },
    ];
    let json = serde_json::to_string(&Gates::parse(&docs, &[]).unwrap().doc()).unwrap();
    assert_eq!(
        json,
        r#"{"gates":[{"name":"proof","evidence":["proof.md"]},{"name":"tests","checks":["make lint"],"approval":"owner","max_tries":3}]}"#
    );
    assert_eq!(
        Gates::from_json(&json).unwrap(),
        Gates::parse(&docs, &[]).unwrap()
    );
}

#[test]
fn json_that_is_not_gates_is_refused() {
    for text in [
        r#"{"gates":[{"name":"a","evidence":["x"],"bogus":1}]}"#,
        "{}",
        r#"{"gates":[]}"#,
    ] {
        assert!(Gates::from_json(text).is_err(), "{text}");
    }
}

#[test]
fn a_line_is_tagged_by_its_kind_and_reads_back() {
    let lines = [
        GateLine::Submitted {
            gate: StepId::parse("proof").unwrap(),
            try_n: 1,
            report: StepReport {
                step: StepId::parse("proof").unwrap(),
                result: StepResult::Ok,
                evidence: vec!["proof.md".to_owned()],
                note: String::new(),
            },
        },
        GateLine::Answered {
            gate: StepId::parse("proof").unwrap(),
            try_n: 1,
            by: "owner".to_owned(),
            answer: Answer::Yes,
            note: String::new(),
        },
        GateLine::Quiet {
            gate: StepId::parse("proof").unwrap(),
            try_n: 2,
        },
    ];
    let kinds: Vec<String> = lines
        .iter()
        .map(|l| {
            serde_json::to_value(l)
                .unwrap()
                .get("kind")
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(kinds, ["submitted", "answered", "quiet"]);
    for line in lines {
        let back: GateLine = serde_json::from_str(&serde_json::to_string(&line).unwrap()).unwrap();
        assert_eq!(back, line);
    }
}
