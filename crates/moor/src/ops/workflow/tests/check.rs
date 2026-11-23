//! Check steps through the tick: the command goes through the exec op in the Box's work place
//! (the fake driver runs it on the host), and its exit decides where the run goes.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "test assertions index JSON values and panic on a wrong shape"
)]

use std::cell::RefCell;

use moor_core::config::UserConfig;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{DriverError, ExecOutcome, Place};
use moor_core::workflow::{
    Plan, RunEnd, RunOutcome, RunState, TimeoutMinutes, WorkflowDef, WorkflowId, WorkflowName,
    WorkflowSchedule,
};
use serde_json::json;

use super::plan::{step_of, PROMPT};
use super::previous::put_to_sleep;
use super::{repo, rig, rig_driving, tick, Rig, FIRST_DUE};
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;

fn plan_of(steps: &[serde_json::Value]) -> Plan {
    Plan::parse(&json!({ "steps": steps }).to_string(), PROMPT).unwrap()
}

fn audit() -> serde_json::Value {
    json!({"id": "audit", "title": "Audit", "kind": "work", "instructions": "look",
           "done_when": "looked", "evidence": ["a.md"], "ok": "gate"})
}

fn gate(command: &str, tries: u8, fail: Option<&str>) -> serde_json::Value {
    json!({"id": "gate", "title": "Gate", "kind": "check", "command": command,
           "max_tries": tries, "ok": "ship", "fail": fail})
}

fn ship() -> serde_json::Value {
    json!({"id": "ship", "title": "Ship", "kind": "deliver",
           "instructions": "ship it", "done_when": "shipped"})
}

impl Rig {
    /// Starts the run of a planned workflow, whose agent is working on its first step.
    fn started(&self, plan: Plan) -> WorkflowId {
        self.answer_repo();
        let id = self.put_planned(Some(plan));
        self.advance_to(FIRST_DUE + 1_000);
        assert!(self.pass().is_empty());
        id
    }

    /// The agent reports the step it is at as done, goes idle, and stays idle long enough for a
    /// check to run. Nothing ticks.
    fn report_and_idle(&self, id: WorkflowId) -> Vec<String> {
        let told = self.report_current(id);
        assert!(!told.error, "{told:?}");
        self.quiet_after_work(id);
        self.pause(3 * 60_000);
        Vec::new()
    }

    /// The history lines of the newest run.
    fn lines(&self, id: WorkflowId) -> Vec<serde_json::Value> {
        let run = self.latest(id);
        self.cx
            .store()
            .unwrap()
            .run_history(run.id)
            .unwrap()
            .into_iter()
            .map(|l| l.entry)
            .collect()
    }
}

fn cut() -> DriverError {
    DriverError::Transport {
        stderr_tail: "the stream ended before the command did".to_owned(),
    }
}

/// Runs a gate whose exec ends as `tune` says, to the line the first check writes.
fn first_line(tune: impl FnOnce(FakeDriver) -> FakeDriver) -> serde_json::Value {
    let r = rig_driving(UserConfig::default(), tune);
    let id = r.started(plan_of(&[audit(), gate("echo probe", 1, None), ship()]));
    r.report_and_idle(id);
    r.pass();
    r.lines(id)[4].clone()
}

#[test]
fn a_machine_that_is_not_linked_is_a_lost_stream() {
    let line = first_line(|d| {
        d.exec_fails_once_when(
            "probe",
            DriverError::Unavailable("machine 1 is not linked".to_owned()),
        )
    });
    assert_eq!(line["kind"], "check_lost", "{line}");
}

#[test]
fn a_driver_error_that_is_no_cut_fails_the_check_with_its_reason() {
    for err in [
        DriverError::Timeout(std::time::Duration::from_secs(5)),
        DriverError::Unavailable("provider is down".to_owned()),
        DriverError::Failed {
            summary: "the box command is empty".to_owned(),
        },
    ] {
        let line = first_line(|d| d.exec_fails_once_when("probe", err.clone()));
        assert_eq!(line["kind"], "checked", "{err:?}: {line}");
        assert_eq!(line["exit"], -2, "{line}");
        let tail = line["tail"].as_str().unwrap();
        assert!(tail.contains("did not run to a result"), "{tail}");
        assert!(tail.contains(&err.to_string()), "{err:?}: {tail}");
    }
}

#[test]
fn an_interrupted_command_with_no_deadline_fails_the_check_and_is_not_lost() {
    let line = first_line(|d| d.exec_ends_once_when("probe", ExecOutcome::Interrupted));
    assert_eq!(line["kind"], "checked", "{line}");
    assert_eq!(line["exit"], -2, "{line}");
    let tail = line["tail"].as_str().unwrap();
    assert!(tail.contains("interrupted before it exited"), "{tail}");
}

#[test]
fn a_tick_that_read_history_before_a_finished_check_does_not_run_it_again() {
    let r = rig();
    let id = r.started(plan_of(&[audit(), gate("true", 1, None), ship()]));
    r.report_and_idle(id);
    let first = RefCell::new(Vec::new());
    tick(
        &r.cx,
        (&|_| {}, &|job| first.borrow_mut().push(job)),
        &|| false,
    );
    assert_eq!(first.borrow().len(), 1);
    // The check finishes while the next tick is between its history read and its claim: its
    // line is written and its flight dropped.
    let finished = RefCell::new(first.into_inner());

    let _hook = super::super::advance::tests_hook::on_history(move |cx| {
        for job in finished.borrow_mut().drain(..) {
            crate::ops::workflow::run_check(cx, &job);
        }
    });
    let second = RefCell::new(0_usize);
    tick(&r.cx, (&|_| {}, &|_| *second.borrow_mut() += 1), &|| false);
    assert_eq!(*second.borrow(), 0);
}

#[test]
fn a_check_that_exits_zero_runs_in_the_work_place_and_the_run_goes_on() {
    let r = rig();
    let id = r.started(plan_of(&[audit(), gate("echo all good", 1, None), ship()]));
    assert!(r.report_and_idle(id).is_empty());
    assert_eq!(step_of(&r.latest(id).state), ("gate".to_owned(), 1));
    assert_eq!(r.kinds(id), ["queued", "entered", "reported", "entered"]);

    assert!(r.pass().is_empty());
    assert_eq!(
        r.kinds(id),
        ["queued", "entered", "reported", "entered", "checked"]
    );
    let checked = r.lines(id)[4].clone();
    assert_eq!(checked["exit"], 0);
    assert!(checked["tail"].as_str().unwrap().contains("all good"));
    let driver = &r.driver;
    let places = driver.places("exec_piped");
    assert_eq!(places, [Place::Work]);

    assert!(r.pass().is_empty());
    assert_eq!(step_of(&r.latest(id).state), ("ship".to_owned(), 1));
    assert_eq!(
        r.kinds(id),
        ["queued", "entered", "reported", "entered", "checked", "entered", "sent"]
    );
}

#[test]
fn a_check_does_not_run_while_the_agent_works() {
    let r = rig();
    let id = r.started(plan_of(&[audit(), gate("true", 1, None), ship()]));
    r.report_and_idle(id);
    assert_eq!(step_of(&r.latest(id).state), ("gate".to_owned(), 1));
    r.cx.store()
        .unwrap()
        .apply(Decision::new(r.latest_box(id)).session(SessionInput::AgentBusy))
        .unwrap();
    r.pause(10 * 60_000);
    r.pass();
    assert_eq!(r.kinds(id), ["queued", "entered", "reported", "entered"]);
}

#[test]
fn a_check_that_exits_non_zero_fails_the_run_when_it_has_no_fail_edge() {
    let r = rig();
    let id = r.started(plan_of(&[
        audit(),
        gate("echo broken; exit 3", 1, None),
        ship(),
    ]));
    r.report_and_idle(id);
    r.pass();
    r.pass();
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Failed { at, reason },
        ..
    }) = r.latest(id).state
    else {
        panic!("{:?}", r.latest(id).state);
    };
    assert_eq!(at.as_str(), "gate");
    assert!(reason.contains("exited 3"), "{reason}");
    let tail = r.lines(id)[4]["tail"].as_str().unwrap().to_owned();
    assert!(tail.contains("broken"), "{tail}");
}

#[test]
fn a_fix_and_check_loop_runs_out_of_tries_and_the_run_fails() {
    let fix = json!({"id": "fix", "title": "Fix", "kind": "work", "max_tries": 2,
        "instructions": "fix it", "done_when": "fixed", "evidence": ["f.md"], "ok": "gate"});
    let plan = plan_of(&[
        json!({"id": "audit", "title": "Audit", "kind": "work", "instructions": "look",
               "done_when": "looked", "evidence": ["a.md"], "ok": "gate"}),
        gate("exit 1", 3, Some("fix")),
        fix,
        ship(),
    ]);
    let r = rig();
    let id = r.started(plan);
    r.report_and_idle(id);
    r.pass();
    r.pass();
    assert_eq!(step_of(&r.latest(id).state), ("fix".to_owned(), 1));
    r.report_and_idle(id);
    r.pass();
    r.pass();
    assert_eq!(step_of(&r.latest(id).state), ("fix".to_owned(), 2));
    r.report_and_idle(id);
    r.pass();
    r.pass();
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Failed { at, reason },
        ..
    }) = r.latest(id).state
    else {
        panic!("{:?}", r.latest(id).state);
    };
    assert_eq!(at.as_str(), "fix");
    assert!(reason.contains("ran out of tries"), "{reason}");
    let kinds = r.kinds(id);
    assert_eq!(
        kinds.iter().filter(|k| *k == "checked").count(),
        3,
        "{kinds:?}"
    );
}

#[test]
fn a_lost_stream_is_no_result_and_the_next_tick_runs_the_check_again() {
    let r = rig_driving(UserConfig::default(), |d| {
        d.exec_fails_once_when("lost-once", cut())
    });
    let id = r.started(plan_of(&[audit(), gate("echo lost-once", 1, None), ship()]));
    r.report_and_idle(id);
    r.pass();
    assert_eq!(
        r.kinds(id),
        ["queued", "entered", "reported", "entered", "check_lost"]
    );
    assert_eq!(step_of(&r.latest(id).state), ("gate".to_owned(), 1));
    r.pass();
    assert_eq!(
        r.kinds(id),
        [
            "queued",
            "entered",
            "reported",
            "entered",
            "check_lost",
            "checked"
        ]
    );
    r.pass();
    assert_eq!(step_of(&r.latest(id).state), ("ship".to_owned(), 1));
}

#[test]
fn two_lost_streams_in_one_try_follow_fail() {
    let r = rig_driving(UserConfig::default(), |d| {
        d.exec_fails_when("lost-always", cut())
    });
    let id = r.started(plan_of(&[
        audit(),
        gate("echo lost-always", 1, None),
        ship(),
    ]));
    r.report_and_idle(id);
    r.pass();
    r.pass();
    assert_eq!(step_of(&r.latest(id).state), ("gate".to_owned(), 1));
    r.pass();
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Failed { at, reason },
        ..
    }) = r.latest(id).state
    else {
        panic!("{:?}", r.latest(id).state);
    };
    assert_eq!(at.as_str(), "gate");
    assert!(reason.contains("lost"), "{reason}");
    assert_eq!(
        r.kinds(id),
        [
            "queued",
            "entered",
            "reported",
            "entered",
            "check_lost",
            "check_lost",
            "ended"
        ]
    );
}

#[test]
fn a_checks_deadline_is_the_smaller_of_the_run_remaining_and_thirty_minutes() {
    let deadline = |minutes: u32| {
        let r = rig();
        r.answer_repo();
        let mut def = WorkflowDef::new(
            WorkflowName::parse("deps").unwrap(),
            Some(WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap()),
            vec![repo()],
            PROMPT,
            (true, TimeoutMinutes::new(minutes).unwrap()),
        )
        .unwrap();
        def.plan = Some(plan_of(&[audit(), gate("true", 1, None), ship()]));
        let store = r.cx.store().unwrap();
        let id = store.put_workflow(REQUESTER_USER, &def).unwrap();
        r.advance_to(FIRST_DUE + 1_000);
        r.pass();
        r.report_and_idle(id);
        r.pause(60_000);
        let jobs = RefCell::new(Vec::new());
        tick(
            &r.cx,
            (&|_| {}, &|job| jobs.borrow_mut().push(job.timeout_secs)),
            &|| false,
        );
        jobs.into_inner()
    };
    assert_eq!(deadline(180), [1_800]);
    let short = deadline(10);
    assert_eq!(short.len(), 1);
    assert!(short[0] <= 600 && short[0] > 300, "{short:?}");
}

#[test]
fn a_check_in_flight_is_not_started_twice() {
    let r = rig();
    let id = r.started(plan_of(&[audit(), gate("true", 1, None), ship()]));
    r.report_and_idle(id);
    let jobs = RefCell::new(Vec::new());
    for _ in 0..2 {
        tick(
            &r.cx,
            (&|_| {}, &|job| jobs.borrow_mut().push(job)),
            &|| false,
        );
    }
    assert_eq!(jobs.borrow().len(), 1);
}

#[test]
fn an_ended_agent_on_a_work_step_is_sent_the_step_again_and_a_second_end_is_incomplete() {
    let r = rig();
    let id = r.started(plan_of(&[audit(), gate("true", 1, None), ship()]));
    let box_id = r.latest_box(id);
    let end = || {
        r.cx.store()
            .unwrap()
            .apply(Decision::new(box_id).session(SessionInput::End))
            .ok();
    };
    end();
    r.pause(3 * 60_000);
    assert!(r.pass().is_empty());
    assert_eq!(step_of(&r.latest(id).state), ("audit".to_owned(), 1));
    assert_eq!(r.kinds(id), ["queued", "entered", "reminded"]);

    end();
    r.pause(3 * 60_000);
    r.pass();
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Incomplete { at, .. },
        ..
    }) = r.latest(id).state
    else {
        panic!("{:?}", r.latest(id).state);
    };
    assert_eq!(at.as_str(), "audit");
}

fn assert_delivered(r: &Rig, id: WorkflowId) {
    let run = r.latest(id);
    assert!(
        matches!(
            run.state,
            RunState::Done(RunEnd::Ended {
                outcome: RunOutcome::Delivered { .. },
                ..
            })
        ),
        "{:?} {:?}",
        run.state,
        r.kinds(id)
    );
}

#[test]
fn a_run_in_flight_still_delivers_after_its_workflow_is_disabled() {
    let r = rig();
    let id = r.started(plan_of(&[audit(), gate("true", 1, None), ship()]));
    r.cx.store()
        .unwrap()
        .set_workflow_enabled(REQUESTER_USER, &WorkflowName::parse("deps").unwrap(), false)
        .unwrap();
    r.report_and_idle(id);
    r.pass();
    r.pass();
    assert_eq!(step_of(&r.latest(id).state), ("ship".to_owned(), 1));
    r.report_and_idle(id);
    assert_delivered(&r, id);
}

#[test]
fn a_box_that_falls_asleep_after_its_agent_reported_a_work_step_still_gets_its_check() {
    let r = rig();
    let id = r.started(plan_of(&[audit(), gate("true", 1, None), ship()]));
    assert!(!r.report_current(id).error);
    r.quiet_after_work(id);
    put_to_sleep(&r, r.latest_box(id));
    r.pause(3 * 60_000);
    r.pass();
    assert_eq!(
        r.kinds(id),
        ["queued", "entered", "reported", "entered", "checked"]
    );
}

#[test]
fn a_box_that_falls_asleep_with_no_report_at_the_deliver_step_is_reminded() {
    let r = rig();
    let id = r.started(plan_of(&[audit(), gate("true", 1, None), ship()]));
    r.report_and_idle(id);
    r.pass();
    r.pass();
    assert_eq!(step_of(&r.latest(id).state), ("ship".to_owned(), 1));
    r.quiet_after_work(id);
    put_to_sleep(&r, r.latest_box(id));
    r.pause(3 * 60_000);
    r.pass();
    assert_eq!(step_of(&r.latest(id).state), ("ship".to_owned(), 1));
    assert_eq!(r.kinds(id).last().map(String::as_str), Some("reminded"));
}
