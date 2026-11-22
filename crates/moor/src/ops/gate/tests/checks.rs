//! The gate pass: checks run once the agent is quiet, in order, and a failure goes back to it.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "test assertions index what they just counted, and one test walks a whole story"
)]

use moor_core::gate::{GateLine, GateNext, GateStatus};
use moor_core::progress::Progress;
use moor_core::workflow::StepId;

use moor_core::workflow::{StepReport, StepResult};

use super::fx::{cut, done, evidence_gate, fx, fx_on, gate};
use crate::testing::driver::FakeDriver;

fn id(name: &str) -> StepId {
    StepId::parse(name).unwrap()
}

fn open(gate: &str, try_n: u8) -> GateStatus {
    GateStatus::Open {
        gate: id(gate),
        try_n,
        next: GateNext::RunChecks,
    }
}

#[test]
fn check_fails_then_passes() {
    let f = fx(|dir| {
        let marker = dir.join("fixed");
        let check = format!("echo not yet; test -e {}", marker.display());
        vec![gate("tests", &[&check], 2)]
    });
    assert_eq!(f.standing(), (open("tests", 1), Progress::Working));

    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(f.exits(), [("tests".to_owned(), Some(1))]);
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(
        told[0].contains("not yet"),
        "the tail goes back: {}",
        told[0]
    );
    assert!(told[0].contains("tests"), "{}", told[0]);
    assert_eq!(f.standing(), (open("tests", 2), Progress::Working));

    f.pause(60);
    assert!(f.pass().is_empty());
    assert_eq!(f.exits().len(), 1, "no check runs while the agent works");

    f.fix();
    f.quiet();
    assert!(f.pass().is_empty());
    assert_eq!(
        f.exits(),
        [("tests".to_owned(), Some(1)), ("tests".to_owned(), Some(0))]
    );
    assert_eq!(f.told().len(), 1, "a pass is not told");
    assert_eq!(f.standing(), (done(), Progress::Done));
}

#[test]
fn checks_run_one_after_another_and_stop_at_the_first_failure() {
    let f = fx(|dir| {
        let log = dir.join("ran");
        let step = |n: &str, exit: u8| format!("echo {n} >> {}; exit {exit}", log.display());
        vec![gate(
            "tests",
            &[&step("a", 0), &step("b", 3), &step("c", 0)],
            1,
        )]
    });
    f.quiet();
    f.pass();
    f.pass();
    f.pass();
    assert_eq!(
        f.exits(),
        [("tests".to_owned(), Some(0)), ("tests".to_owned(), Some(3))]
    );
    let ran = std::fs::read_to_string(f.dir.path().join("ran")).unwrap();
    assert_eq!(ran, "a\nb\n", "the third check never ran");
}

#[test]
fn the_second_gate_is_checked_only_after_the_first_passed() {
    let f = fx(|dir| {
        let first = format!("test -e {}", dir.join("fixed").display());
        let second = format!("echo second > {}", dir.join("second").display());
        vec![gate("first", &[&first], 3), gate("second", &[&second], 1)]
    });
    f.quiet();
    f.pass();
    assert_eq!(f.exits(), [("first".to_owned(), Some(1))]);
    f.quiet();
    f.pass();
    assert!(!f.dir.path().join("second").exists());
    assert_eq!(f.standing().0, open("first", 3));

    f.fix();
    f.quiet();
    f.pass();
    assert_eq!(f.standing().0, open("second", 1));
    f.pass();
    assert_eq!(
        f.exits(),
        [
            ("first".to_owned(), Some(1)),
            ("first".to_owned(), Some(1)),
            ("first".to_owned(), Some(0)),
            ("second".to_owned(), Some(0)),
        ]
    );
    assert!(f.dir.path().join("second").exists());
    assert_eq!(f.standing(), (done(), Progress::Done));
}

#[test]
fn a_lost_stream_runs_the_check_again() {
    let f = fx_on(
        FakeDriver::new().exec_fails_once_when("lost-once", cut()),
        |_| vec![gate("tests", &["echo lost-once"], 1)],
    );
    f.quiet();
    f.pass();
    assert_eq!(f.exits(), [("tests".to_owned(), None)]);
    assert_eq!(
        f.standing().0,
        open("tests", 1),
        "a lost stream is no result"
    );
    f.pass();
    assert_eq!(
        f.exits(),
        [("tests".to_owned(), None), ("tests".to_owned(), Some(0))]
    );
    assert_eq!(f.standing(), (done(), Progress::Done));
    assert!(f.told().is_empty());
}

#[test]
fn two_lost_streams_in_one_try_fail_the_try() {
    let f = fx_on(
        FakeDriver::new().exec_fails_when("lost-always", cut()),
        |_| vec![gate("tests", &["echo lost-always"], 1)],
    );
    f.quiet();
    f.pass();
    f.pass();
    f.pass();
    assert_eq!(
        f.exits(),
        [("tests".to_owned(), None), ("tests".to_owned(), None)]
    );
    let (status, progress) = f.standing();
    assert!(
        matches!(&status, GateStatus::Stuck { why, .. } if why.contains("lost")),
        "{status:?}"
    );
    assert_eq!(progress, Progress::NeedsYou);
}

#[test]
fn a_try_that_failed_with_tries_left_tells_the_agent_that_the_output_was_lost() {
    let f = fx_on(
        FakeDriver::new().exec_fails_when("lost-always", cut()),
        |_| vec![gate("tests", &["echo lost-always"], 2)],
    );
    f.quiet();
    f.pass();
    f.pass();
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("lost"), "{}", told[0]);
    assert_eq!(f.standing().0, open("tests", 2));
}

#[test]
fn a_check_does_not_run_while_the_agent_works() {
    let f = fx(|_| vec![gate("tests", &["true"], 1)]);
    assert!(f.due().is_empty(), "the agent is still on its first turn");
    f.quiet();
    assert_eq!(f.due().len(), 1);
}

#[test]
fn a_check_in_flight_is_not_started_twice() {
    let f = fx(|_| vec![gate("tests", &["true"], 1)]);
    f.quiet();
    let first = f.due();
    assert_eq!(first.len(), 1);
    assert!(f.due().is_empty(), "its flight holds the Box");
    drop(first);
    assert_eq!(f.due().len(), 1);
}

#[test]
fn of_two_passes_that_read_the_same_lines_one_writes_the_check() {
    let f = fx(|_| vec![gate("tests", &["true"], 1)]);
    f.quiet();
    let jobs = f.due();
    let job = jobs.first().unwrap();
    assert!(crate::ops::gate::checks::run(&f.cx, job).is_empty());
    assert!(
        crate::ops::gate::checks::run(&f.cx, job).is_empty(),
        "the loser drops its result without a word"
    );
    let lines = f.lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(matches!(&lines[0], GateLine::Checked { exit: 0, .. }));
}

#[test]
fn a_check_failure_not_yet_told_is_told_although_a_refusal_came_after_it() {
    let f = fx(|dir| {
        let check = format!("test -e {}", dir.join("fixed").display());
        vec![evidence_gate("proof", &["proof.md"], &[&check], 2)]
    });
    let store = f.cx.store().unwrap();
    let write = |n: i64, line: GateLine| {
        store
            .append_gate_line(f.id, n, f.cx.clock().now(), &line)
            .unwrap();
        f.pause(1);
    };
    let proof = id("proof");
    let report = StepReport {
        step: proof.clone(),
        result: StepResult::Ok,
        evidence: vec!["proof.md".to_owned()],
        note: String::new(),
    };
    write(
        1,
        GateLine::Submitted {
            gate: proof.clone(),
            try_n: 1,
            report,
        },
    );
    write(
        2,
        GateLine::Checked {
            gate: proof.clone(),
            try_n: 1,
            check: 0,
            exit: 1,
            tail: "nope".to_owned(),
        },
    );
    write(
        3,
        GateLine::Refused {
            gate: proof,
            try_n: 2,
            missing: vec!["proof.md".to_owned()],
        },
    );
    f.quiet();
    assert!(f.pass().is_empty());
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("nope"), "{}", told[0]);
    assert!(told[0].contains("call report_step again"), "{}", told[0]);
    f.pass();
    assert_eq!(f.told().len(), 1, "told once");
}

#[test]
fn a_check_sees_the_env_of_the_run_the_agent_shell_sees() {
    let f = fx(|_| {
        vec![gate(
            "posted",
            &["test \"$MOOR_PULL_NUMBER\" = 65 && test \"$MOOR_PULL_REPO\" = o/n"],
            1,
        )]
    });
    let pull = ("MOOR_PULL_NUMBER", "65".to_owned());
    let repo = ("MOOR_PULL_REPO", "o/n".to_owned());
    let over = moor_core::config::with_env(None, &[pull, repo]).unwrap();
    f.cx.store()
        .unwrap()
        .set_box_config_override(f.id, Some(&over))
        .unwrap();
    f.quiet();
    f.pass();
    assert_eq!(f.exits(), [("posted".to_owned(), Some(0))]);
}
