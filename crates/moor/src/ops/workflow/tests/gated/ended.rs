//! The Box of a gated run that ended: its gates no longer decide what it shows, and neither a
//! steer nor a report moves them.

use moor_core::gate::GateLine;
use moor_core::progress::Progress;
use moor_core::request::{BoxTarget, StepReportReq};
use moor_core::view::BoxView;
use moor_core::workflow::{StepReport, StepResult};
use moor_core::{BoxId, ShowReq, SteerReq};

use super::super::hook::{cancelled, hooked, Hooked, NEW, OLD};
use super::{gone_quiet, proof, ship, step};
use crate::ops::gate::checks::{pass, run, Job};

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
fn shown(r: &super::super::Rig, id: BoxId) -> BoxView {
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

#[test]
fn the_box_of_a_run_whose_gate_failed_for_good_reads_failed_with_the_reason() {
    let r = super::super::rig();
    let once = moor_core::gate::GateDoc {
        max_tries: Some(1),
        ..ship()
    };
    let id = r.started_gated(&[once]);
    let box_id = r.latest_box(id);
    let no = GateLine::Answered {
        gate: step("ship"),
        try_n: 1,
        by: "me".to_owned(),
        answer: moor_core::workflow::Answer::No,
        note: "not like this".to_owned(),
    };
    r.gate_line(id, &no);
    r.quiet_after_work(id);
    r.pass();
    assert!(
        r.latest(id).state.phase().as_str() == "done",
        "the run ended"
    );
    let progress = shown(&r, box_id).progress.unwrap();
    assert_eq!(progress.state, Progress::Failed, "{progress:?}");
    assert!(progress.line.contains("not like this"), "{progress:?}");
}

/// A gated pull request run with `gates` and `outcomes` whose agent went quiet, so a gate pass
/// handed out a job, and that a newer head cancelled before the job ran; its Box and the job.
fn cancelled_with_a_job_due(
    gates: &[moor_core::gate::GateDoc],
    outcomes: &[moor_core::gate::OutcomeDoc],
) -> (Hooked, BoxId, Job) {
    let r = hooked();
    let gates = moor_core::gate::Gates::parse(gates, outcomes).unwrap();
    let id = r.put_gates(gates, true);
    r.signed("opened", 65, OLD);
    let old = r.latest_box(id);
    r.quiet_after_work(id);
    let jobs = std::cell::RefCell::new(Vec::new());
    let store = r.cx.store().unwrap();
    pass(&r.cx, store, &|| false, &|job| jobs.borrow_mut().push(job)).unwrap();
    let job = jobs.into_inner().pop().expect("a job is due");
    r.pause(1_000);
    r.signed("synchronize", 65, NEW);
    assert!(cancelled(&r.one_at(id, 65, OLD)).is_some());
    (r, old, job)
}

/// Runs `job` and asserts it wrote no line and steered no one in Box `old`.
fn ran_without_a_trace(r: &Hooked, old: BoxId, job: &Job) {
    let store = r.cx.store().unwrap();
    let lines = store.gate_lines(old).unwrap();
    let steered = store.newest_event_of(old, "steered").unwrap();
    let said = run(&r.cx, job);
    assert!(
        said.iter().any(|l| l.contains("its run has ended")),
        "the drop is logged with its reason: {said:?}"
    );
    assert_eq!(store.gate_lines(old).unwrap(), lines, "nothing is recorded");
    assert_eq!(
        store.newest_event_of(old, "steered").unwrap().map(|e| e.id),
        steered.map(|e| e.id),
        "nobody is told"
    );
}

#[test]
fn a_gate_check_that_ends_after_its_run_was_cancelled_leaves_no_trace() {
    let tests = moor_core::gate::GateDoc {
        name: "tests".to_owned(),
        evidence: Vec::new(),
        checks: vec!["false".into()],
        approval: None,
        max_tries: Some(2),
    };
    let (r, old, job) = cancelled_with_a_job_due(&[tests], &[]);
    ran_without_a_trace(&r, old, &job);
}

#[test]
fn an_outcome_that_holds_after_its_run_was_cancelled_leaves_no_trace() {
    let anytime: moor_core::gate::OutcomeDoc = serde_json::from_value(serde_json::json!(
        {"name": "nothing to do", "checks": ["true"], "anytime": true}
    ))
    .unwrap();
    let done = serde_json::from_value(serde_json::json!({"name": "done"})).unwrap();
    let (r, old, job) = cancelled_with_a_job_due(&[proof()], &[anytime, done]);
    ran_without_a_trace(&r, old, &job);
}

#[test]
fn a_reminder_due_after_its_run_was_cancelled_is_not_sent() {
    let anytime: moor_core::gate::OutcomeDoc = serde_json::from_value(serde_json::json!(
        {"name": "nothing to do", "checks": ["false"], "anytime": true}
    ))
    .unwrap();
    let done = serde_json::from_value(serde_json::json!({"name": "done"})).unwrap();
    let (r, old, job) = cancelled_with_a_job_due(&[proof()], &[anytime, done]);
    ran_without_a_trace(&r, old, &job);
}

#[test]
fn a_job_line_for_the_box_of_an_ended_run_is_not_written() {
    let tests = moor_core::gate::GateDoc {
        name: "tests".to_owned(),
        evidence: Vec::new(),
        checks: vec!["false".into()],
        approval: None,
        max_tries: Some(2),
    };
    let (r, old, _job) = cancelled_with_a_job_due(&[tests], &[]);
    let store = r.cx.store().unwrap();
    let lines = store.gate_lines(old).unwrap();
    let line = moor_core::gate::GateLine::Reminded {
        gate: step("tests"),
        try_n: 1,
    };
    let n = i64::try_from(lines.len()).unwrap() + 1;
    let wrote = store.append_job_line(old, n, r.cx.clock().now(), &line);
    assert_eq!(wrote.unwrap(), crate::store::Appended::RunEnded);
    assert_eq!(store.gate_lines(old).unwrap(), lines);
}
