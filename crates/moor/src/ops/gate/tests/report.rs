//! The agent's report for a gate: evidence is checked, the answer says what happens next, and a
//! report that is not for the gate in hand records nothing.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "test assertions index what they just counted, and one test walks a whole story"
)]

use moor_core::gate::{GateLine, GateNext, GateStatus};
use moor_core::progress::Progress;
use moor_core::workflow::{StepId, StepResult};

use super::fx::{done, evidence_gate, fx, gate};

fn id(name: &str) -> StepId {
    StepId::parse(name).unwrap()
}

fn open(gate: &str, try_n: u8, next: GateNext) -> GateStatus {
    GateStatus::Open {
        gate: id(gate),
        try_n,
        next,
    }
}

fn kinds(lines: &[GateLine]) -> Vec<&'static str> {
    lines
        .iter()
        .map(|l| match l {
            GateLine::Submitted { .. } => "submitted",
            GateLine::Refused { .. } => "refused",
            GateLine::Checked { .. } => "checked",
            GateLine::Reminded { .. } => "reminded",
            GateLine::Quiet { .. } => "quiet",
            GateLine::TryFailed { .. } => "try_failed",
            _ => "other",
        })
        .collect()
}

/// The reasons of the Box's `report_refused` events, oldest first.
fn refusals(f: &super::fx::Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter()
        .filter(|e| e.kind == "report_refused")
        .filter_map(|e| Some(e.payload.get("reason")?.as_str()?.to_owned()))
        .collect()
}

#[test]
fn every_refused_report_is_recorded_with_its_reason_and_an_accepted_one_is_not() {
    let f = fx(|_| vec![evidence_gate("proof", &["proof.md"], &[], 2)]);
    let elsewhere = f.report("shots", StepResult::Ok, &[]);
    f.leave(&[]);
    let missing = f.report("proof", StepResult::Ok, &["proof.md"]);
    assert!(
        elsewhere.error && missing.error,
        "{elsewhere:?} {missing:?}"
    );
    f.leave(&[("proof.md", 5)]);
    let ok = f.report("proof", StepResult::Ok, &["proof.md"]);
    assert!(!ok.error, "{ok:?}");
    assert_eq!(refusals(&f), [elsewhere.reply, missing.reply]);
}

#[test]
fn a_report_with_every_named_file_passes_a_gate_that_needs_nothing_more() {
    let f = fx(|_| vec![evidence_gate("proof", &["proof.md", "shots-*"], &[], 1)]);
    f.leave(&[("proof.md", 5), ("shots-1.png", 9)]);
    let told = f.report("proof", StepResult::Ok, &["proof.md", "shots-1.png"]);
    assert!(!told.error, "{told:?}");
    assert!(told.reply.contains("passed"), "{told:?}");
    assert_eq!(kinds(&f.lines()), ["submitted"]);
    assert_eq!(f.standing().0, done());
}

#[test]
fn a_report_that_is_in_order_moves_the_gate_on_to_its_checks_and_the_next_gate() {
    let f = fx(|_| {
        vec![
            evidence_gate("proof", &["proof.md"], &["true"], 1),
            evidence_gate("shots", &["shot.png"], &[], 1),
        ]
    });
    f.leave(&[("proof.md", 5)]);
    let told = f.report("proof", StepResult::Ok, &["proof.md"]);
    assert!(!told.error, "{told:?}");
    assert!(told.reply.contains("moor runs the checks"), "{told:?}");
    assert_eq!(
        f.standing(),
        (open("proof", 1, GateNext::RunChecks), Progress::Working)
    );

    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(f.standing().0, open("shots", 1, GateNext::Agent));
    f.leave(&[("shot.png", 3)]);
    let told = f.report("shots", StepResult::Ok, &["shot.png"]);
    assert!(!told.error, "{told:?}");
    assert_eq!(f.standing().0, done());
}

#[test]
fn a_report_missing_a_file_is_refused_by_name_and_a_second_refusal_fails_the_try() {
    let f = fx(|_| vec![evidence_gate("proof", &["proof.md"], &[], 2)]);
    f.leave(&[]);
    let told = f.report("proof", StepResult::Ok, &["proof.md"]);
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("proof.md"), "{told:?}");
    assert!(told.reply.contains("second refusal"), "{told:?}");
    assert_eq!(kinds(&f.lines()), ["refused"]);
    assert_eq!(f.standing().0, open("proof", 1, GateNext::Agent));

    f.leave(&[("proof.md", 0)]);
    let told = f.report("proof", StepResult::Ok, &["proof.md"]);
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("`proof.md` is empty"), "{told:?}");
    assert!(told.reply.contains("try 1 failed"), "{told:?}");
    assert_eq!(f.standing().0, open("proof", 2, GateNext::Agent));

    f.leave(&[]);
    f.report("proof", StepResult::Ok, &["proof.md"]);
    f.leave(&[]);
    let told = f.report("proof", StepResult::Ok, &["proof.md"]);
    assert!(told.reply.contains("out of tries"), "{told:?}");
    let (status, progress) = f.standing();
    assert!(matches!(status, GateStatus::Stuck { .. }), "{status:?}");
    assert_eq!(progress, Progress::NeedsYou);
}

#[test]
fn a_report_that_is_not_for_the_gate_in_hand_is_refused_and_records_nothing() {
    let f = fx(|_| {
        vec![
            evidence_gate("proof", &["proof.md"], &[], 1),
            evidence_gate("shots", &["shot.png"], &[], 1),
        ]
    });
    let other = f.report("shots", StepResult::Ok, &["shot.png"]);
    assert!(other.error, "{other:?}");
    assert!(other.reply.contains("gate in hand is `proof`"), "{other:?}");
    let unknown = f.report("nothing", StepResult::Ok, &[]);
    assert!(unknown.error, "{unknown:?}");
    assert!(unknown.reply.contains("no gate `nothing`"), "{unknown:?}");
    assert!(unknown.reply.contains("proof, shots"), "{unknown:?}");
    assert!(f.lines().is_empty());
}

#[test]
fn a_box_with_no_gates_and_no_run_is_refused_as_before() {
    let f = fx(|_| vec![gate("tests", &["true"], 1)]);
    let plain = super::new_box(&f.cx, "plain");
    let told = f.report_from(plain, "proof", StepResult::Ok, &[]);
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("no running workflow run"), "{told:?}");
    assert!(f.lines().is_empty());
}

#[test]
fn a_gate_that_takes_no_report_says_who_runs_it() {
    let f = fx(|_| vec![gate("tests", &["true"], 1)]);
    let told = f.report("tests", StepResult::Ok, &[]);
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("moor runs"), "{told:?}");
    assert!(f.lines().is_empty());
}

#[test]
fn a_report_repeated_after_a_lost_reply_is_already_recorded() {
    let f = fx(|_| vec![evidence_gate("proof", &["proof.md"], &["true"], 1)]);
    f.leave(&[("proof.md", 5)]);
    f.report("proof", StepResult::Ok, &["proof.md"]);
    let again = f.report("proof", StepResult::Ok, &["proof.md"]);
    assert!(!again.error, "{again:?}");
    assert!(again.reply.contains("Already recorded"), "{again:?}");
    assert_eq!(kinds(&f.lines()), ["submitted"]);
}

#[test]
fn a_fail_report_with_a_note_fails_the_try() {
    let f = fx(|_| vec![evidence_gate("proof", &["proof.md"], &[], 1)]);
    let told = f.report("proof", StepResult::Fail, &[]);
    assert!(!told.error, "{told:?}");
    let (status, progress) = f.standing();
    assert!(matches!(status, GateStatus::Stuck { .. }), "{status:?}");
    assert_eq!(progress, Progress::NeedsYou);
}

#[test]
fn a_failing_check_tells_the_agent_to_fix_and_report_again() {
    let f = fx(|dir| {
        let check = format!("echo nope; test -e {}", dir.join("fixed").display());
        vec![evidence_gate("proof", &["proof.md"], &[&check], 2)]
    });
    f.leave(&[("proof.md", 5)]);
    f.report("proof", StepResult::Ok, &["proof.md"]);
    f.quiet();
    assert!(f.pass().is_empty());
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("nope"), "{}", told[0]);
    assert!(told[0].contains("call report_step again"), "{}", told[0]);
    assert!(!told[0].contains("once you are quiet"), "{}", told[0]);
    assert_eq!(f.standing().0, open("proof", 2, GateNext::Agent));

    f.fix();
    f.leave(&[("proof.md", 5)]);
    f.report("proof", StepResult::Ok, &["proof.md"]);
    f.quiet();
    f.pass();
    assert_eq!(f.standing().0, done());
}
