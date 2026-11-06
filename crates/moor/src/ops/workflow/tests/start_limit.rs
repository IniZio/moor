//! A run has two limits: an hour from its claim to its agent starting, and `timeout_minutes`
//! from the agent starting.
#![allow(clippy::panic, reason = "a test names what it found when it fails")]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::num::NonZeroU32;

use moor_core::config::{NodeConfig, UserConfig};
use moor_core::name::ForgeRepo;
use moor_core::request::CloseReq;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    TimeoutMinutes, WorkflowDef, WorkflowId, WorkflowName, WorkflowOutcome, WorkflowRun,
    WorkflowSchedule,
};
use moor_core::{BoxId, StartReq};

use super::{rig, rig_with, Rig, FIRST_DUE};
use crate::ops::workflow::{resolve_interrupted, run_start, tick, StartJob};
use crate::testing::forge::repo_at;

const MIN: i64 = 60_000;
const CLAIMED: i64 = FIRST_DUE + 1_000;

fn other() -> ForgeRepo {
    ForgeRepo::new("o", "m")
}

/// A workflow of repos `o/n` and `o/m` with `timeout` minutes, on a one-Box node.
fn two_repos(r: &Rig, timeout: u32) -> WorkflowId {
    r.answer_repo();
    r.forge.answer(other(), Ok(repo_at("abc123", None)));
    let def = WorkflowDef::new(
        WorkflowName::parse("daily-fix").unwrap(),
        WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap(),
        vec![super::repo(), other()],
        "fix what is broken",
        (true, TimeoutMinutes::new(timeout).unwrap()),
    )
    .unwrap();
    r.cx.store()
        .unwrap()
        .put_workflow(crate::testing::cx::REQUESTER_USER, &def)
        .unwrap()
}

fn one_box_node() -> UserConfig {
    UserConfig {
        nodes: BTreeMap::from([(
            "local-container".to_owned(),
            NodeConfig {
                runtime: "local-container".to_owned(),
                labels: Vec::new(),
                max_boxes: Some(NonZeroU32::new(1).unwrap()),
            },
        )]),
        ..UserConfig::default()
    }
}

fn runs_of(r: &Rig, id: WorkflowId, repo: &ForgeRepo) -> Vec<WorkflowRun> {
    r.cx.store()
        .unwrap()
        .recent_workflow_runs(id, repo, 50)
        .unwrap()
}

fn latest_of(r: &Rig, id: WorkflowId, repo: &ForgeRepo) -> WorkflowRun {
    runs_of(r, id, repo).into_iter().next().unwrap()
}

/// Claims the firing at the first due time and returns its jobs without starting any Box.
fn claim_only(r: &Rig) -> Vec<StartJob> {
    r.advance_to(CLAIMED);
    let jobs = RefCell::new(Vec::new());
    tick(&r.cx, &|j| jobs.borrow_mut().push(j), &|| false);
    jobs.into_inner()
}

fn launched_at(r: &Rig, id: BoxId) -> Ts {
    r.cx.store()
        .unwrap()
        .first_launch_at(id)
        .unwrap()
        .expect("the agent started")
}

fn refused_detail(run: &WorkflowRun) -> String {
    match &run.outcome {
        WorkflowOutcome::Refused { detail } => detail.clone(),
        other => panic!("not refused: {other:?}"),
    }
}

fn close_box(r: &Rig, id: BoxId) {
    let store = r.cx.store().unwrap();
    let name = store.box_row(id).unwrap().unwrap().name;
    let req = CloseReq {
        target: name.into(),
        force: true,
    };
    crate::ops::close::run(&r.cx, &req).unwrap();
}

#[test]
fn a_setup_longer_than_the_timeout_keeps_its_agent_and_the_timeout_counts_from_the_agent() {
    let r = rig();
    r.answer_repo();
    let id = r.put(10);
    let job = claim_only(&r).remove(0);
    r.advance_to(CLAIMED + 18 * MIN);
    run_start(&r.cx, &job);
    let box_id = r.latest(id).outcome.box_id().unwrap();
    let agent = launched_at(&r, box_id).as_millis();
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Started { .. }
    ));
    assert!(r
        .cx
        .store()
        .unwrap()
        .live_session(box_id)
        .unwrap()
        .is_some());
    r.advance_to(agent + 10 * MIN - 1);
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Started { .. }
    ));
    r.advance_to(agent + 10 * MIN);
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
fn a_queued_box_past_the_start_limit_is_closed_and_its_run_refused() {
    let r = rig_with(one_box_node());
    let id = two_repos(&r, 720);
    r.advance_to(CLAIMED);
    r.pass();
    let queued = latest_of(&r, id, &other());
    let queued_box = queued
        .outcome
        .box_id()
        .expect("queued Box recorded as started");
    assert!(r
        .cx
        .store()
        .unwrap()
        .first_launch_at(queued_box)
        .unwrap()
        .is_none());

    r.advance_to(CLAIMED + 59 * MIN);
    r.pass();
    assert!(matches!(
        latest_of(&r, id, &other()).outcome,
        WorkflowOutcome::Started { .. }
    ));

    r.advance_to(CLAIMED + 61 * MIN);
    r.pass();
    let detail = refused_detail(&latest_of(&r, id, &other()));
    assert!(
        detail.contains("did not start within 60 minutes"),
        "{detail}"
    );
    assert_eq!(
        r.open_boxes(),
        1,
        "only the Box that started its agent stays"
    );
    assert!(r
        .cx
        .store()
        .unwrap()
        .first_launch_at(queued_box)
        .unwrap()
        .is_none());
    assert!(matches!(
        latest_of(&r, id, &super::repo()).outcome,
        WorkflowOutcome::Started { .. }
    ));
}

#[test]
fn the_next_firing_runs_after_a_start_limit_breach() {
    let r = rig_with(one_box_node());
    let id = two_repos(&r, 720);
    r.advance_to(CLAIMED);
    r.pass();
    r.advance_to(CLAIMED + 61 * MIN);
    r.pass();
    let breached = latest_of(&r, id, &other());
    r.advance_to(super::FIRST_DUE + super::DAY + 1_000);
    r.pass();
    let next = latest_of(&r, id, &other());
    assert!(next.due_at > breached.due_at);
    assert!(
        !matches!(next.outcome, WorkflowOutcome::SkippedOverlap),
        "{next:?}"
    );
}

#[test]
fn a_pending_box_admitted_in_time_gets_its_full_timeout_from_its_agent() {
    let r = rig_with(one_box_node());
    let id = two_repos(&r, 10);
    r.advance_to(CLAIMED);
    r.pass();
    let holder = latest_of(&r, id, &super::repo()).outcome.box_id().unwrap();
    let waiting = latest_of(&r, id, &other()).outcome.box_id().unwrap();
    r.advance_to(CLAIMED + 30 * MIN);
    close_box(&r, holder);
    let name =
        r.cx.store()
            .unwrap()
            .box_row(waiting)
            .unwrap()
            .unwrap()
            .name;
    crate::ops::start::run(
        &r.cx,
        &StartReq {
            name,
            prompt: Some("fix what is broken".to_owned()),
            issue: None,
            base: None,
            model: None,
            node: None,
            no_agent: false,
            resume: false,
            repo: Some(other()),
            also: None,
            parent: None,
            identity: None,
        },
    )
    .unwrap();
    let agent = launched_at(&r, waiting).as_millis();
    assert!(agent >= CLAIMED + 30 * MIN);
    r.advance_to(agent + 9 * MIN);
    r.pass();
    assert!(matches!(
        latest_of(&r, id, &other()).outcome,
        WorkflowOutcome::Started { .. }
    ));
    r.advance_to(agent + 10 * MIN);
    r.pass();
    assert!(matches!(
        latest_of(&r, id, &other()).outcome,
        WorkflowOutcome::TimedOut { .. }
    ));
}

#[test]
fn a_claim_with_no_box_past_the_limit_is_refused_and_a_late_start_changes_nothing() {
    let r = rig();
    r.answer_repo();
    let id = r.put(10);
    let job = claim_only(&r).remove(0);
    r.advance_to(CLAIMED + 59 * MIN);
    r.pass();
    assert_eq!(r.latest(id).outcome, WorkflowOutcome::Starting);
    r.advance_to(CLAIMED + 61 * MIN);
    r.pass();
    let detail = refused_detail(&r.latest(id));
    assert!(
        detail.contains("did not start within 60 minutes"),
        "{detail}"
    );

    let late = run_start(&r.cx, &job);
    assert_eq!(refused_detail(&r.latest(id)), detail, "{late:?}");
    assert_eq!(r.open_boxes(), 0, "the late Box is closed again: {late:?}");
}

#[test]
fn a_box_still_setting_up_past_the_limit_is_closed_and_its_run_refused() {
    let r = rig();
    r.answer_repo();
    let id = r.put(10);
    let job = claim_only(&r).remove(0);
    let req = StartReq {
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
    };
    let made = crate::ops::start::run(&r.cx, &req).unwrap();
    assert_eq!(r.open_boxes(), 1);
    r.advance_to(CLAIMED + 61 * MIN);
    r.pass();
    assert!(refused_detail(&r.latest(id)).contains("did not start within"));
    assert_eq!(r.open_boxes(), 0);
    assert!(r
        .cx
        .store()
        .unwrap()
        .live_session(BoxId::new(made.boxed.id))
        .unwrap()
        .is_none());
}

#[test]
fn a_claim_settled_at_boot_keeps_its_claim_time_and_counts_the_run_from_its_agent() {
    let r = rig();
    r.answer_repo();
    let id = r.put(10);
    let job = claim_only(&r).remove(0);
    let made = crate::ops::start::run(
        &r.cx,
        &StartReq {
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
    let agent = launched_at(&r, BoxId::new(made.boxed.id)).as_millis();
    r.advance_to(agent + 25 * MIN);
    resolve_interrupted(&r.cx);
    let run = r.latest(id);
    assert!(matches!(run.outcome, WorkflowOutcome::Started { .. }));
    assert_eq!(
        run.recorded_at,
        Ts::from_millis(CLAIMED),
        "the claim time is kept"
    );
    r.pass();
    assert!(
        matches!(r.latest(id).outcome, WorkflowOutcome::TimedOut { .. }),
        "the agent has worked past the timeout, whenever the controller restarted"
    );
}
