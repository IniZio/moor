//! The pass decides the outcome by running the outcomes' checks: at the closing stage once every
//! gate passed, and for an `anytime` outcome when a gate waits for a quiet agent.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    clippy::panic,
    reason = "test assertions index what they just counted, and one test walks a whole story"
)]

use std::path::Path;

use moor_core::gate::{
    closing, CheckDoc, GateDoc, GateLine, GateNext, GateStatus, OutcomeDoc, Tone,
};
use moor_core::progress::Progress;

use super::fx::{busy, cut, evidence_gate, fx_outcomes, fx_outcomes_on, gate, Fx};
use crate::testing::driver::FakeDriver;

/// An outcome that holds once the file `name` is in `dir`.
fn when(dir: &Path, name: &str, tone: Option<Tone>, anytime: bool) -> OutcomeDoc {
    OutcomeDoc {
        name: name.to_owned(),
        says: None,
        tone,
        checks: vec![CheckDoc::from(format!(
            "echo no {name}; test -e {}",
            dir.join(name.replace(' ', "-")).display()
        ))],
        anytime,
    }
}

/// The fixture's three outcomes over `gates`.
fn three(dir: &Path, gates: Vec<GateDoc>) -> (Vec<GateDoc>, Vec<OutcomeDoc>) {
    let outcomes = vec![
        when(dir, "nothing to do", Some(Tone::Plain), true),
        when(dir, "check failed", Some(Tone::Bad), false),
        when(dir, "check passed", None, false),
    ];
    (gates, outcomes)
}

/// Makes outcome `name` hold.
fn hold(f: &Fx, name: &str) {
    std::fs::write(f.dir.path().join(name.replace(' ', "-")), "").unwrap();
}

fn kinds(f: &Fx) -> Vec<String> {
    f.lines()
        .iter()
        .map(|l| {
            let v = serde_json::to_value(l).unwrap();
            v["kind"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn reached(f: &Fx) -> (String, Tone, Progress) {
    match f.standing() {
        (GateStatus::Passed { outcome }, progress) => {
            (outcome.name.to_string(), outcome.tone, progress)
        }
        other => panic!("not passed: {other:?}"),
    }
}

/// A Box whose one gate runs `true`, then decides among the three outcomes.
fn closing_box() -> Fx {
    fx_outcomes(|dir| three(dir, vec![gate("tests", &["true"], 2)]))
}

/// The agent goes quiet and the pass runs the gate's check, then the outcomes'.
fn through(f: &Fx) {
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(
        f.standing().0,
        GateStatus::Open {
            gate: closing(),
            try_n: 1,
            next: GateNext::Decide,
        }
    );
    assert!(f.pass().is_empty());
}

#[test]
fn once_every_gate_passed_the_first_outcome_that_holds_ends_the_run() {
    let f = closing_box();
    hold(&f, "check passed");
    through(&f);
    assert_eq!(
        reached(&f),
        ("check passed".to_owned(), Tone::Good, Progress::Done)
    );
    assert_eq!(kinds(&f), ["checked", "held"]);
    assert!(f.told().is_empty(), "a held outcome is not steered");
}

#[test]
fn a_failed_pull_request_check_ends_the_run_done_in_a_bad_outcome() {
    let f = closing_box();
    hold(&f, "check failed");
    hold(&f, "check passed");
    through(&f);
    assert_eq!(
        reached(&f),
        ("check failed".to_owned(), Tone::Bad, Progress::Done),
        "outcomes are tried in order"
    );
}

#[test]
fn an_anytime_outcome_ends_the_run_when_the_agent_is_quiet_with_nothing_handed_in() {
    let f = fx_outcomes(|dir| {
        let gates = vec![
            evidence_gate("proof", &["pr-*"], &[], 2),
            evidence_gate("done", &["posted.md"], &["true"], 2),
        ];
        three(dir, gates)
    });
    hold(&f, "nothing to do");
    f.pause(60);
    assert!(f.pass().is_empty());
    assert!(
        f.lines().is_empty(),
        "nothing is tried while the agent works"
    );
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(
        reached(&f),
        ("nothing to do".to_owned(), Tone::Plain, Progress::Done)
    );
    assert_eq!(kinds(&f), ["held"]);
    assert!(f.told().is_empty(), "no reminder is sent");
}

#[test]
fn with_no_anytime_outcome_holding_a_quiet_agent_is_reminded_then_stuck_as_before() {
    let f = fx_outcomes(|dir| three(dir, vec![evidence_gate("proof", &["pr-*"], &[], 1)]));
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(kinds(&f), ["reminded"]);
    assert_eq!(f.told().len(), 1);
    assert!(f.pass().is_empty());
    assert_eq!(kinds(&f), ["reminded"], "one reminder, not one per pass");
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(kinds(&f), ["reminded", "quiet"]);
    assert!(matches!(f.standing().0, GateStatus::Stuck { .. }));
    hold(&f, "nothing to do");
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(
        kinds(&f),
        ["reminded", "quiet"],
        "a stuck gate tries nothing"
    );
}

#[test]
fn no_outcome_holding_is_steered_then_stuck_and_a_person_reopens_it() {
    let f = closing_box();
    through(&f);
    assert_eq!(kinds(&f), ["checked", "none_held"]);
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("no outcome holds"), "{}", told[0]);
    assert!(
        told[0].contains("no check passed"),
        "the tail goes back: {}",
        told[0]
    );
    assert_eq!(
        f.standing(),
        (
            GateStatus::Open {
                gate: closing(),
                try_n: 2,
                next: GateNext::Decide,
            },
            Progress::Working
        )
    );
    f.quiet();
    assert!(f.pass().is_empty());
    let (status, progress) = f.standing();
    assert!(
        matches!(&status, GateStatus::Stuck { gate, try_n: 2, .. } if *gate == closing()),
        "{status:?}"
    );
    assert_eq!(progress, Progress::NeedsYou);
    assert_eq!(f.told().len(), 1, "out of tries waits for a person");

    f.steer("the status is set now, look again");
    hold(&f, "check passed");
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(
        reached(&f),
        ("check passed".to_owned(), Tone::Good, Progress::Done)
    );
    assert!(matches!(
        f.lines().as_slice(),
        [
            ..,
            GateLine::Reopened { .. },
            GateLine::Held { try_n: 1, .. }
        ]
    ));
}

#[test]
fn an_outcome_check_that_did_not_run_decides_nothing_and_runs_on_the_next_pass() {
    let f = fx_outcomes_on(
        FakeDriver::new().exec_fails_once_when("no check failed", busy()),
        |dir| {
            let outcomes = vec![
                when(dir, "check failed", Some(Tone::Bad), false),
                OutcomeDoc {
                    name: "check passed".to_owned(),
                    says: None,
                    tone: None,
                    checks: Vec::new(),
                    anytime: false,
                },
            ];
            (vec![gate("tests", &["true"], 2)], outcomes)
        },
    );
    hold(&f, "check failed");
    f.quiet();
    f.pass();
    let logged = f.pass();
    assert!(logged.iter().any(|l| l.contains("busy")), "{logged:?}");
    assert_eq!(
        kinds(&f),
        ["checked", "no_answer"],
        "no result decides nothing"
    );
    assert!(f.told().is_empty());
    f.pass();
    assert_eq!(
        reached(&f),
        ("check failed".to_owned(), Tone::Bad, Progress::Done),
        "a check that did not run never lets a later outcome win"
    );
}

#[test]
fn a_lost_stream_of_an_outcome_check_decides_nothing_and_runs_on_the_next_pass() {
    let f = fx_outcomes_on(
        FakeDriver::new().exec_fails_once_when("no check failed", cut()),
        |dir| three(dir, vec![gate("tests", &["true"], 2)]),
    );
    hold(&f, "check failed");
    f.quiet();
    f.pass();
    f.pass();
    assert_eq!(
        kinds(&f),
        ["checked", "no_answer"],
        "a lost stream decides nothing"
    );
    assert!(
        f.told().is_empty(),
        "the agent is not steered for moor's fault"
    );
    f.pass();
    assert_eq!(
        reached(&f),
        ("check failed".to_owned(), Tone::Bad, Progress::Done)
    );
}

#[test]
fn a_lost_stream_of_an_anytime_check_does_not_remind() {
    let f = fx_outcomes_on(
        FakeDriver::new().exec_fails_once_when("no nothing to do", cut()),
        |dir| three(dir, vec![evidence_gate("proof", &["pr-*"], &[], 2)]),
    );
    hold(&f, "nothing to do");
    f.quiet();
    f.pass();
    assert_eq!(kinds(&f), ["no_answer"]);
    assert!(
        f.told().is_empty(),
        "no reminder for a check that gave no answer"
    );
    f.pass();
    assert_eq!(
        reached(&f),
        ("nothing to do".to_owned(), Tone::Plain, Progress::Done)
    );
}

/// A Box whose one gate has only a check, which fails until the agent fixes the work.
fn checks_only_box() -> Fx {
    fx_outcomes(|dir| {
        let check = format!("test -e {}", dir.join("fixed").display());
        three(dir, vec![gate("tests", &[&check], 2)])
    })
}

#[test]
fn an_anytime_outcome_is_tried_before_the_checks_of_a_gate_that_has_only_checks() {
    let f = checks_only_box();
    hold(&f, "nothing to do");
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(
        reached(&f),
        ("nothing to do".to_owned(), Tone::Plain, Progress::Done)
    );
    assert_eq!(kinds(&f), ["held"], "the gate's check never ran");
}

#[test]
fn with_no_anytime_outcome_holding_the_gates_check_runs_in_the_same_pass() {
    let f = checks_only_box();
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(kinds(&f), ["checked"]);
    assert_eq!(f.told().len(), 1, "the failed check goes to the agent");
    hold(&f, "nothing to do");
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(
        reached(&f),
        ("nothing to do".to_owned(), Tone::Plain, Progress::Done),
        "tried again on the next try"
    );
}

#[test]
fn a_lost_anytime_check_at_a_gate_with_only_checks_holds_back_the_gates_check_too() {
    let f = fx_outcomes_on(
        FakeDriver::new().exec_fails_once_when("no nothing to do", cut()),
        |dir| three(dir, vec![gate("tests", &["true"], 2)]),
    );
    f.quiet();
    f.pass();
    assert_eq!(kinds(&f), ["no_answer"], "the gate's check did not run");
    f.pass();
    assert_eq!(
        kinds(&f),
        ["no_answer", "checked"],
        "the next pass runs both"
    );
}
