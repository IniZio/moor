//! Tests for the move a report makes, the second refusal and the code gate.
#![allow(clippy::panic, reason = "test assertions panic on a wrong shape")]

use serde_json::json;

use super::{id, plan, work};
use crate::artifact::ArtifactFile;
use crate::decide::workflow::{code_gate, refusal_move, report_move, Move};
use crate::workflow::{HistoryEntry, Plan, StepReport, StepResult, EVIDENCE_MAX, NOTE_MAX};

fn entered(step: &str, try_n: u8) -> HistoryEntry {
    HistoryEntry::Entered {
        step: id(step),
        try_n,
    }
}

fn refused(step: &str, try_n: u8) -> HistoryEntry {
    HistoryEntry::GateRefused {
        step: id(step),
        try_n,
        missing: vec!["x".to_owned()],
    }
}

fn say(step: &str, result: StepResult, note: &str) -> StepReport {
    StepReport {
        step: id(step),
        result,
        evidence: Vec::new(),
        note: note.to_owned(),
    }
}

fn file(name: &str, size: u64) -> ArtifactFile {
    ArtifactFile {
        name: name.to_owned(),
        size,
        sha256: "a".repeat(64),
    }
}

#[test]
fn an_ok_report_follows_the_ok_edge_and_a_fail_report_the_fail_edge() {
    let p = plan(&[work("a", "b", Some("end"), 1), work("b", "end", None, 1)]);
    let h = [entered("a", 1)];
    let Move::Enter { step, .. } = report_move(&p, &h, &say("a", StepResult::Ok, "")) else {
        panic!("ok should go on");
    };
    assert_eq!(step, id("b"));
    let Move::Enter { step, .. } = report_move(&p, &h, &say("a", StepResult::Fail, "no")) else {
        panic!("fail should take its edge");
    };
    assert_eq!(step, id("end"));
}

#[test]
fn a_fail_report_with_no_fail_edge_fails_the_run_with_the_note() {
    let p = plan(&[work("a", "end", None, 1)]);
    let h = [entered("a", 1)];
    let Move::Fail { at, reason, .. } = report_move(&p, &h, &say("a", StepResult::Fail, " why "))
    else {
        panic!("the run should fail");
    };
    assert_eq!(at, id("a"));
    assert!(reason.contains("failed: why"), "{reason}");
}

#[test]
fn a_deliver_step_delivers_on_ok_and_fails_the_run_on_fail() {
    let p = plan(&[]);
    let h = [entered("end", 1)];
    assert_eq!(
        report_move(&p, &h, &say("end", StepResult::Ok, "done")),
        Move::Delivered
    );
    assert!(matches!(
        report_move(&p, &h, &say("end", StepResult::Fail, "broke")),
        Move::Fail { .. }
    ));
}

#[test]
fn the_second_refusal_of_a_try_fails_the_step_and_the_first_waits() {
    let p = plan(&[work("a", "end", Some("end"), 1)]);
    assert_eq!(refusal_move(&p, &[entered("a", 1)]), Move::Wait);
    let h = [entered("a", 1), refused("a", 1)];
    let Move::Enter { step, .. } = refusal_move(&p, &h) else {
        panic!("a second refusal takes the fail edge");
    };
    assert_eq!(step, id("end"));
    let h = [refused("a", 1), entered("a", 2)];
    assert_eq!(
        refusal_move(&p, &h),
        Move::Wait,
        "a refusal belongs to its try"
    );
}

fn gated(kind: &str, report: &StepReport, index: &[ArtifactFile]) -> Result<(), Vec<String>> {
    let p = Plan::parse(
        &json!({"steps": [
            {"id": "a", "title": "A", "kind": "work", "instructions": "i", "done_when": "d",
             "evidence": ["flat.md", "shots-*"], "ok": "end"},
            {"id": "end", "title": "End", "kind": "deliver", "instructions": "i", "done_when": "d"}
        ]})
        .to_string(),
        "p",
    )
    .unwrap();
    code_gate(p.step(&id(kind)).unwrap(), report, index)
}

#[test]
fn the_gate_wants_each_evidence_name_as_a_non_empty_file_within_the_limit() {
    let ok = say("a", StepResult::Ok, "");
    let full = [
        file("flat.md", 3),
        file("shots-1.png", 1),
        file("shots-2.png", 9),
    ];
    assert_eq!(gated("a", &ok, &full), Ok(()));

    let missing = gated("a", &ok, &[file("flat.md", 3)]).unwrap_err();
    assert_eq!(missing, ["no file matches `shots-*` in .moor/artifacts/"]);

    let empty = gated(
        "a",
        &ok,
        &[file("flat.md", 0), file("shots-1", 2), file("shots-2", 0)],
    );
    assert_eq!(
        empty.unwrap_err(),
        ["`flat.md` is empty", "`shots-2` is empty"]
    );

    let huge = [
        file("flat.md", EVIDENCE_MAX),
        file("shots-1", EVIDENCE_MAX + 1),
    ];
    let err = gated("a", &ok, &huge).unwrap_err();
    assert_eq!(err.len(), 1, "{err:?}");
    assert!(
        err[0].contains("`shots-1`") && err[0].contains("25 MiB"),
        "{err:?}"
    );

    let other = [file("flat.md.bak", 3), file("shots", 3)];
    assert_eq!(gated("a", &ok, &other).unwrap_err().len(), 2);
}

#[test]
fn a_fail_report_passes_the_gate_with_a_note_and_needs_no_files() {
    let fail = |note: &str| say("a", StepResult::Fail, note);
    assert_eq!(gated("a", &fail("the registry is down"), &[]), Ok(()));
    assert!(gated("a", &fail("  "), &[]).is_err());
}

#[test]
fn a_deliver_report_needs_a_note_and_a_note_is_capped() {
    assert!(gated("end", &say("end", StepResult::Ok, ""), &[]).is_err());
    assert_eq!(
        gated("end", &say("end", StepResult::Ok, "PR 3"), &[]),
        Ok(())
    );
    let long = "x".repeat(NOTE_MAX + 1);
    let err = gated("end", &say("end", StepResult::Ok, &long), &[]).unwrap_err();
    assert!(err[0].contains("at most 2048"), "{err:?}");
}
