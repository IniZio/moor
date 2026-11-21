//! The Box of a gated run that ended: its gates no longer decide what it shows, and neither a
//! steer nor a report moves them.

use moor_core::gate::GateLine;
use moor_core::progress::Progress;
use moor_core::request::{BoxTarget, StepReportReq};
use moor_core::view::BoxView;
use moor_core::workflow::{StepReport, StepResult};
use moor_core::{BoxId, ShowReq, SteerReq};

use super::super::hook::{cancelled, hooked, Hooked, NEW, OLD};
use super::{gone_quiet, proof, step};

/// A gated pull request run whose agent went quiet at its gate, stuck there when `stuck`, and
/// that a newer head then cancelled; its Box.
fn cancelled_at_its_gate(stuck: bool) -> (Hooked, BoxId) {
    let r = hooked();
    let id = r.put_gated_for(&[proof()], true);
    r.signed("opened", 65, OLD);
    let old = r.latest_box(id);
    if stuck {
        gone_quiet(id, &r, "proof");
    }
    r.quiet_after_work(id);
    r.pause(1_000);
    r.signed("synchronize", 65, NEW);
    assert!(cancelled(&r.one_at(id, 65, OLD)).is_some());
    (r, old)
}

/// Box `id` as `show` lists it.
fn shown(r: &Hooked, id: BoxId) -> BoxView {
    let req = ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
        transcript: false,
    };
    let listed = crate::ops::show::run(&r.cx, &req).unwrap().boxes.unwrap();
    listed.into_iter().find(|b| b.id == id.get()).unwrap()
}

#[test]
fn the_box_of_a_cancelled_run_reads_cancelled_not_working() {
    let (r, old) = cancelled_at_its_gate(false);
    let progress = shown(&r, old).progress.unwrap();
    assert_ne!(progress.state, Progress::Working, "{progress:?}");
    assert!(progress.line.contains("cancelled"), "{progress:?}");
}

#[test]
fn a_persons_steer_does_not_reopen_the_stuck_gate_of_an_ended_run() {
    let (r, old) = cancelled_at_its_gate(true);
    let req = SteerReq {
        target: BoxTarget::Id(old),
        text: Some("try again".to_owned()),
        menu: None,
        interrupt: false,
        ask: None,
    };
    crate::ops::steer::run(&r.cx, &req).unwrap();
    let lines = r.cx.store().unwrap().gate_lines(old).unwrap();
    assert!(
        !lines.iter().any(|l| matches!(l, GateLine::Reopened { .. })),
        "{lines:?}"
    );
}

#[test]
fn a_report_from_the_box_of_an_ended_run_is_refused_readably() {
    let (r, old) = cancelled_at_its_gate(false);
    let store = r.cx.store().unwrap();
    let before = store.gate_lines(old).unwrap();
    let report = StepReport {
        step: step("proof"),
        result: StepResult::Ok,
        evidence: vec!["pr-1.md".to_owned()],
        note: String::new(),
    };
    let req = StepReportReq {
        box_id: old,
        report,
    };
    let reply = crate::ops::workflow::step_report(&r.cx, &req).unwrap();
    assert!(reply.error, "{reply:?}");
    assert!(reply.reply.contains("this run has ended"), "{reply:?}");
    assert_eq!(
        store.gate_lines(old).unwrap(),
        before,
        "nothing is recorded"
    );
}
