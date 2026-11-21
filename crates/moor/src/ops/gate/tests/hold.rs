//! A Box whose gates are not passed: a person's steer reopens a stuck gate, and auto-close leaves
//! it open.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "test assertions index what they just counted, and one test walks a whole story"
)]

use std::time::Duration;

use moor_core::config::AutoClose;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::gate::{GateLine, GateNext, GateStatus};
use moor_core::progress::Progress;
use moor_core::seam::clock::Ts;
use moor_core::workflow::StepId;

use super::fx::{fx, gate, Fx};
use crate::ops::{auto_close_pass, AutoCloseLooked};

const DAY: u64 = 86_400;

fn open(try_n: u8) -> GateStatus {
    GateStatus::Open {
        gate: StepId::parse("tests").unwrap(),
        try_n,
        next: GateNext::RunChecks,
    }
}

fn failing(f: &Fx) -> (GateStatus, Progress) {
    f.quiet();
    f.pass();
    f.standing()
}

#[test]
fn tries_used_up_is_stuck_and_a_steer_reopens_the_gate() {
    let f = fx(|dir| {
        let check = format!("echo nope; test -e {}", dir.join("fixed").display());
        vec![gate("tests", &[&check], 1)]
    });
    let (status, progress) = failing(&f);
    assert!(
        matches!(&status, GateStatus::Stuck { why, .. } if why.contains("out of tries")),
        "{status:?}"
    );
    assert_eq!(progress, Progress::NeedsYou);
    assert!(f.told().is_empty(), "a stuck gate is for a person");
    f.pass();
    assert_eq!(f.exits().len(), 1, "a stuck gate runs no more checks");

    f.steer("try it this way");
    let lines = f.lines();
    assert!(
        matches!(lines.last(), Some(GateLine::Reopened { by, .. }) if !by.is_empty()),
        "{lines:?}"
    );
    assert_eq!(f.standing(), (open(1), Progress::Working));

    f.fix();
    f.quiet();
    f.pass();
    assert_eq!(f.standing(), (GateStatus::Passed, Progress::Done));
}

#[test]
fn a_steer_of_a_gate_that_is_not_stuck_writes_nothing() {
    let f = fx(|_| vec![gate("tests", &["false"], 2)]);
    f.steer("how is it going");
    assert!(f.lines().is_empty());
    f.quiet();
    f.pass();
    assert_eq!(f.standing().0, open(2));
    f.steer("and now");
    assert!(!f
        .lines()
        .iter()
        .any(|l| matches!(l, GateLine::Reopened { .. })));
}

const fn idle_close() -> AutoClose {
    AutoClose {
        every: Duration::from_secs(60),
        run_after: Some(Duration::from_secs(DAY)),
        idle_after: Some(Duration::from_secs(7 * DAY)),
    }
}

/// Lets a week pass and runs the auto-close pass; whether the Box closed.
fn closes_after_a_week(f: &Fx) -> bool {
    f.quiet();
    f.pause(8 * DAY);
    let store = f.cx.store().unwrap();
    let looked = AutoCloseLooked::default();
    auto_close_pass(&f.cx, store, &|| false, (&idle_close(), &looked)).unwrap();
    store.box_row(f.id).unwrap().unwrap().lifecycle == BoxLifecycle::Closed
}

fn write_line(f: &Fx, n: i64, line: &GateLine) {
    let at = Ts::from_millis(n);
    f.cx.store()
        .unwrap()
        .append_gate_line(f.id, n, at, line)
        .unwrap();
}

fn checked(exit: i32) -> GateLine {
    GateLine::Checked {
        gate: StepId::parse("tests").unwrap(),
        try_n: 1,
        check: 0,
        exit,
        tail: String::new(),
    }
}

#[test]
fn auto_close_leaves_an_idle_box_whose_gate_is_open() {
    let f = fx(|_| vec![gate("tests", &["true"], 1)]);
    assert!(!closes_after_a_week(&f));
}

#[test]
fn auto_close_leaves_an_idle_box_whose_gate_is_stuck() {
    let f = fx(|_| vec![gate("tests", &["true"], 1)]);
    write_line(&f, 1, &checked(1));
    assert!(matches!(f.standing().0, GateStatus::Stuck { .. }));
    assert!(!closes_after_a_week(&f));
}

#[test]
fn auto_close_closes_an_idle_box_whose_gate_failed_for_good() {
    let asks = moor_core::gate::GateDoc {
        approval: Some(moor_core::gate::Approval::Owner),
        ..gate("tests", &[], 1)
    };
    let f = fx(|_| vec![asks]);
    let no = GateLine::Answered {
        gate: StepId::parse("tests").unwrap(),
        try_n: 1,
        by: "me".to_owned(),
        answer: moor_core::workflow::Answer::No,
        note: String::new(),
    };
    write_line(&f, 1, &no);
    assert!(matches!(f.standing().0, GateStatus::Failed { .. }));
    assert!(closes_after_a_week(&f));
}

#[test]
fn auto_close_closes_an_idle_box_whose_gates_passed() {
    let f = fx(|_| vec![gate("tests", &["true"], 1)]);
    write_line(&f, 1, &checked(0));
    assert!(closes_after_a_week(&f));
}
