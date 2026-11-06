//! The clock of a run's timeout starts when the run starts, not when it fires.

use std::cell::RefCell;

use moor_core::seam::clock::Ts;
use moor_core::workflow::WorkflowOutcome;

use super::{rig, Rig, FIRST_DUE};
use crate::ops::workflow::{resolve_interrupted, run_start, tick, StartJob};

/// Claims the firing at the first due time and returns its job without starting the Box.
fn claim_only(r: &Rig) -> StartJob {
    r.advance_to(FIRST_DUE + 1_000);
    let job = RefCell::new(None);
    tick(&r.cx, &|j| *job.borrow_mut() = Some(j), &|| false);
    job.into_inner().unwrap()
}

#[test]
fn a_slow_setup_does_not_count_against_the_timeout() {
    let r = rig();
    r.answer_repo();
    let id = r.put(10);
    let job = claim_only(&r);
    r.advance_to(FIRST_DUE + 1_000 + 18 * 60_000);
    run_start(&r.cx, &job);
    let box_id = r.latest_box(id);
    r.pass();
    assert!(
        matches!(r.latest(id).outcome, WorkflowOutcome::Started { .. }),
        "setup took longer than the timeout and the run is still live"
    );
    assert!(r
        .cx
        .store()
        .unwrap()
        .live_session(box_id)
        .unwrap()
        .is_some());

    r.advance_to(FIRST_DUE + 1_000 + 18 * 60_000 + 9 * 60_000);
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Started { .. }
    ));
}

#[test]
fn a_run_is_expired_once_the_timeout_has_passed_since_it_started() {
    let r = rig();
    r.answer_repo();
    let id = r.put(10);
    let job = claim_only(&r);
    r.advance_to(FIRST_DUE + 1_000 + 18 * 60_000);
    run_start(&r.cx, &job);
    let box_id = r.latest_box(id);
    let started_at = r.latest(id).recorded_at.as_millis();
    assert!(started_at >= FIRST_DUE + 1_000 + 18 * 60_000);
    r.advance_to(started_at + 10 * 60_000 - 1);
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Started { .. }
    ));
    r.advance_to(started_at + 10 * 60_000);
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::TimedOut { .. }
    ));
    assert!(r
        .cx
        .store()
        .unwrap()
        .live_session(box_id)
        .unwrap()
        .is_none());
}

#[test]
fn a_claim_settled_at_boot_counts_its_timeout_from_the_boot() {
    let r = rig();
    r.answer_repo();
    let id = r.put(10);
    let job = claim_only(&r);
    crate::ops::start::run(
        &r.cx,
        &moor_core::StartReq {
            name: job.name.clone(),
            prompt: Some(job.prompt.clone()),
            issue: None,
            base: None,
            model: None,
            node: None,
            no_agent: false,
            resume: false,
            repo: Some(job.repo),
            also: None,
            parent: None,
            identity: None,
        },
    )
    .unwrap();
    let boot = FIRST_DUE + 1_000 + 18 * 60_000;
    r.advance_to(boot);
    resolve_interrupted(&r.cx);
    assert_eq!(r.latest(id).recorded_at, Ts::from_millis(boot));
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Started { .. }
    ));
    r.advance_to(boot + 10 * 60_000);
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::TimedOut { .. }
    ));
}
