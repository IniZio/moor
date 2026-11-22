//! The Box of a gated run closed while the run waits or works: the run ends incomplete at its
//! gate the moment the Box closes, and the workflow can run again.

use moor_core::gate::{GateDoc, GateLine};
use moor_core::progress::Progress;
use moor_core::user::Viewer;
use moor_core::view::BoxView;
use moor_core::workflow::{RunOutcome, Unfinished, WorkflowId};
use moor_core::{BoxId, CloseReq, ShowReq};

use super::super::{rig, Rig, DAY, FIRST_DUE};
use super::{delivered, gone_quiet, proof, reported, ship, step, MIN};
use crate::ops::gate::checks::{pass, run};
use crate::testing::cx::REQUESTER_USER;

/// Closes Box `id` as a person acting through `cx` does; the rig's Boxes answer no probe, so with force.
fn close_as(r: &Rig, cx: &crate::cx::Cx, id: BoxId) {
    let name = r.cx.store().unwrap().box_row(id).unwrap().unwrap().name;
    let req = CloseReq {
        target: name.into(),
        force: true,
    };
    crate::ops::close::requested(cx, &req).unwrap();
}

/// Fires the workflow by hand, as its owner, and says whether a run started.
fn run_now(r: &Rig) -> String {
    let start = |job| {
        crate::ops::workflow::run_start(&r.cx, &job);
        Ok(())
    };
    crate::ops::workflow::now::run_with(
        &r.cx,
        r.cx.store().unwrap(),
        REQUESTER_USER,
        ("evidence", None, None),
        (&|_| unreachable!("no pull request is named"), &start),
    )
    .unwrap()
    .status
}

/// Box `id` as `show` lists it.
fn shown(r: &Rig, id: BoxId) -> BoxView {
    let req = ShowReq {
        target: Some(moor_core::request::BoxTarget::Id(id)),
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
        transcript: false,
        everyone: false,
    };
    crate::ops::show::run(&r.cx, &req).unwrap().boxed.unwrap()
}

/// Closes the Box of the newest run and asserts, with no tick between, that the run ended
/// incomplete at `gate` with a reason that says so, and that the workflow runs again.
fn closed_ends_incomplete(r: &Rig, id: WorkflowId, gate: &str) {
    let box_id = r.latest_box(id);
    close_as(r, &r.cx, box_id);
    let incomplete = RunOutcome::Incomplete {
        at: step(gate),
        why: Unfinished::BoxClosed,
    };
    assert_eq!(r.outcome(id), incomplete, "{:?}", r.latest(id).state);
    let told = r.event(id, "run_failed").expect("the owner hears of it");
    let reason = told["reason"].as_str().unwrap();
    assert_eq!(
        reason,
        format!("its Box was closed while the run was at {gate}")
    );
    assert_eq!(run_now(r), "started");
    assert_ne!(r.latest_box(id), box_id, "a new run, in a new Box");
}

#[test]
fn closing_the_box_of_a_run_stuck_at_its_gate_ends_it_incomplete() {
    let r = rig();
    let id = r.started_gated(&[proof()]);
    gone_quiet(id, &r, "proof");
    r.pass();
    assert_eq!(r.at(id).0, "waiting");
    closed_ends_incomplete(&r, id, "proof");
}

#[test]
fn closing_the_box_of_a_run_waiting_for_the_owners_yes_ends_it_incomplete() {
    let r = rig();
    let id = r.started_gated(&[proof(), ship()]);
    r.gate_line(id, &reported("proof", "proof is in"));
    r.quiet_after_work(id);
    r.pause(MIN);
    r.pass();
    assert_eq!(r.at(id), ("waiting", ("ship".to_owned(), 1)));
    closed_ends_incomplete(&r, id, "ship");
}

#[test]
fn closing_the_box_of_a_run_whose_agent_works_at_its_gate_ends_it_incomplete() {
    let r = rig();
    let id = r.started_gated(&[proof()]);
    assert_eq!(r.at(id), ("running", ("proof".to_owned(), 1)));
    closed_ends_incomplete(&r, id, "proof");
}

#[test]
fn the_operator_closing_the_box_ends_its_run_the_same_way() {
    let r = rig();
    let id = r.started_gated(&[proof(), ship()]);
    r.gate_line(id, &reported("proof", "proof is in"));
    r.quiet_after_work(id);
    r.pause(MIN);
    r.pass();
    let operator = r.cx.detached().unwrap().with_viewer(Viewer::Operator);
    close_as(&r, &operator, r.latest_box(id));
    assert!(matches!(
        r.outcome(id),
        RunOutcome::Incomplete {
            why: Unfinished::BoxClosed,
            ..
        }
    ));
}

#[test]
fn the_closed_box_of_a_waiting_run_asks_nobody_and_says_why_it_ended() {
    let r = rig();
    let id = r.started_gated(&[proof(), ship()]);
    r.gate_line(id, &reported("proof", "proof is in"));
    r.quiet_after_work(id);
    r.pause(MIN);
    r.pass();
    let box_id = r.latest_box(id);
    assert!(
        shown(&r, box_id).gate.unwrap().wait.is_some(),
        "it asks before the close"
    );
    close_as(&r, &r.cx, box_id);
    r.pass();
    let b = shown(&r, box_id);
    let gate = b.gate.unwrap();
    assert_eq!(gate.wait, None, "{gate:?}");
    assert_eq!(gate.run_ended.as_deref(), Some("incomplete"));
    let progress = b.progress.unwrap();
    assert_eq!(progress.needs, None, "{progress:?}");
    assert_ne!(progress.state, Progress::NeedsYou, "{progress:?}");
    assert!(
        progress
            .line
            .contains("its Box was closed while the run was at ship"),
        "{progress:?}"
    );
}

#[test]
fn a_gate_check_that_ends_after_the_close_is_dropped() {
    let r = rig();
    let passing = GateDoc {
        name: "tests".to_owned(),
        evidence: Vec::new(),
        checks: vec!["true".into()],
        approval: None,
        max_tries: Some(2),
    };
    let id = r.started_gated(&[passing]);
    let box_id = r.latest_box(id);
    r.quiet_after_work(id);
    let jobs = std::cell::RefCell::new(Vec::new());
    let store = r.cx.store().unwrap();
    pass(&r.cx, store, &|| false, &|job| jobs.borrow_mut().push(job)).unwrap();
    let job = jobs.into_inner().pop().expect("a check is due");
    close_as(&r, &r.cx, box_id);
    let lines = store.gate_lines(box_id).unwrap();
    run(&r.cx, &job);
    let late = GateLine::Checked {
        gate: step("tests"),
        try_n: 1,
        check: 0,
        exit: 0,
        tail: String::new(),
    };
    let n = i64::try_from(lines.len()).unwrap() + 1;
    let wrote = store.append_job_line(box_id, n, r.cx.clock().now(), &late);
    assert_eq!(wrote.unwrap(), crate::store::Appended::RunEnded);
    assert_eq!(
        store.gate_lines(box_id).unwrap(),
        lines,
        "nothing is recorded"
    );
    r.pass();
    assert!(matches!(r.outcome(id), RunOutcome::Incomplete { .. }));
}

#[test]
fn closing_the_box_of_a_run_whose_gates_passed_delivers_it_and_a_later_tick_changes_nothing() {
    let r = rig();
    let id = r.started_gated(&[proof()]);
    r.gate_line(id, &reported("proof", "all green"));
    close_as(&r, &r.cx, r.latest_box(id));
    assert_eq!(r.outcome(id), delivered("all green"));
    r.gate_line(
        id,
        &GateLine::Reopened {
            gate: step("proof"),
            by: "me".to_owned(),
        },
    );
    r.pass();
    assert_eq!(r.outcome(id), delivered("all green"));
    assert!(r.event(id, "run_failed").is_none());
}

#[test]
fn closing_the_box_of_a_run_that_already_ended_changes_nothing() {
    let r = rig();
    let id = r.started_gated(&[proof()]);
    r.gate_line(id, &reported("proof", "all green"));
    r.pass();
    let ended = r.latest(id).state;
    close_as(&r, &r.cx, r.latest_box(id));
    r.pass();
    assert_eq!(r.latest(id).state, ended);
    assert!(r.event(id, "run_failed").is_none());
}

#[test]
fn the_next_cron_firing_after_the_close_starts_a_new_run() {
    let r = rig();
    let id = r.started_gated(&[proof(), ship()]);
    let first = r.latest_box(id);
    close_as(&r, &r.cx, first);
    r.advance_to(FIRST_DUE + DAY + 1_000);
    assert!(r.pass().is_empty());
    assert_ne!(r.latest_box(id), first);
    assert_eq!(r.at(id), ("running", ("proof".to_owned(), 1)));
}
