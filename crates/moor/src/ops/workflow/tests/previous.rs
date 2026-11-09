//! A run's new Box closes the earlier Boxes of the same workflow and repo, and only those.
#![allow(clippy::panic, reason = "a test names what it found when it fails")]

use std::sync::Arc;

use moor_core::config::{BoxConfig, UserConfig};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::workflow::{WorkflowId, WorkflowName};
use moor_core::{BoxId, BoxPath};

use super::{repo, Rig, DAY, FIRST_DUE};
use crate::cx::Cx;
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::FakeForge;
use crate::testing::proc::ScriptedProc;

const CLEAN: &str = "ahead=0 dirty=0\n";

/// A rig whose first Box answers its close check with `first` and every other Box with a clean one.
fn rig_probing(first: &str) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let driver = Arc::new((1..=6).fold(FakeDriver::new(), |d, n| {
        let work = BoxPath::under(&format!("/work/cbx_{n}/repo"), ".moor/boxwork").unwrap();
        let text = if n == 1 { first } else { CLEAN };
        d.peek_text(&format!("box-{n}"), &environ, "HOME=/home/box\0")
            .peek_text(&format!("box-{n}"), &work, text)
    }));
    let forge = Arc::new(FakeForge::new());
    let cx = Cx::fake_with(
        dir.path(),
        driver,
        Arc::new(ScriptedProc::new()),
        UserConfig {
            box_: BoxConfig {
                max_awake: Some("100000h".to_owned()),
                ..BoxConfig::default()
            },
            ..UserConfig::default()
        },
    )
    .with_forge(forge.clone());
    Rig {
        cx,
        forge,
        _dir: dir,
    }
}

fn lifecycle(r: &Rig, id: BoxId) -> BoxLifecycle {
    r.cx.store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .lifecycle
}

fn event_kinds(r: &Rig, id: BoxId) -> Vec<String> {
    r.cx.store()
        .unwrap()
        .events(id, 1000)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

/// Runs day one to its end: the agent goes quiet and the run finishes.
fn finished_first_day(r: &Rig, id: WorkflowId) -> BoxId {
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let first = r.latest_box(id);
    r.cx.store()
        .unwrap()
        .apply(Decision::new(first).session(SessionInput::AgentQuiet))
        .unwrap();
    r.advance_to(FIRST_DUE + 1_000 + 3 * 60_000);
    r.pass();
    first
}

/// Puts the Box to sleep the way `moor sleep` does, which records what it holds, just after its
/// agent was last read.
fn put_to_sleep(r: &Rig, id: BoxId) {
    let store = r.cx.store().unwrap();
    let name = store.box_row(id).unwrap().unwrap().name;
    if let Some(live) = store.live_session(id).unwrap() {
        store.note_agent_read(live.id, r.cx.clock().now()).unwrap();
    }
    crate::ops::sleep::run(&r.cx, &moor_core::BoxReq { box_name: name }).unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(
        row.hands.unwrap().state,
        moor_core::domain::hands::HandsState::Asleep
    );
}

fn next_day(r: &Rig) {
    r.advance_to(FIRST_DUE + DAY + 1_000);
    r.pass();
}

#[test]
fn an_idle_previous_box_is_closed_once_the_new_runs_box_started() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    put_to_sleep(&r, first);
    next_day(&r);
    let second = r.latest_box(id);
    assert_ne!(second, first);
    assert_eq!(lifecycle(&r, first), BoxLifecycle::Closed);
    assert_ne!(lifecycle(&r, second), BoxLifecycle::Closed);
    assert_eq!(r.open_boxes(), 1);
}

#[test]
fn a_previous_box_whose_agent_is_working_is_kept_with_a_reason_on_the_new_box() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    r.cx.store()
        .unwrap()
        .apply(Decision::new(first).session(SessionInput::AgentBusy))
        .unwrap();
    let lines = {
        r.advance_to(FIRST_DUE + DAY + 1_000);
        r.pass()
    };
    let second = r.latest_box(id);
    assert_ne!(lifecycle(&r, first), BoxLifecycle::Closed);
    assert_eq!(r.open_boxes(), 2);
    assert!(event_kinds(&r, second).contains(&"previous_box_kept".to_owned()));
    assert!(
        lines.iter().any(|l| l.contains("its agent is working")),
        "{lines:?}"
    );
}

#[test]
fn a_previous_box_with_unpushed_work_is_kept_and_not_forced() {
    let r = rig_probing("ahead=1 dirty=0\n");
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    put_to_sleep(&r, first);
    next_day(&r);
    let second = r.latest_box(id);
    assert_ne!(lifecycle(&r, first), BoxLifecycle::Closed);
    assert_eq!(r.open_boxes(), 2);
    assert!(event_kinds(&r, second).contains(&"previous_box_kept".to_owned()));
    assert!(!event_kinds(&r, first).contains(&"closed".to_owned()));
    assert!(
        !event_kinds(&r, first).contains(&"woke".to_owned()),
        "a dirty record keeps the Box asleep"
    );
}

/// A Box the owner started on the repo, outside any workflow.
fn start_owner_box(r: &Rig) -> BoxId {
    crate::ops::start::run(
        &r.cx,
        &moor_core::StartReq {
            name: moor_core::BoxName::parse_allow_bare("owner-box").unwrap(),
            prompt: None,
            issue: None,
            base: None,
            model: None,
            node: None,
            no_agent: true,
            cold: false,
            resume: false,
            repo: Some(repo()),
            also: None,
            parent: None,
            identity: None,
        },
    )
    .map(|v| BoxId::new(v.boxed.id))
    .unwrap()
}

#[test]
fn another_workflows_box_and_an_owner_box_are_not_closed() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let mine = r.put(180);
    let other = {
        let def = moor_core::workflow::WorkflowDef::new(
            WorkflowName::parse("nightly-lint").unwrap(),
            moor_core::workflow::WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap(),
            vec![repo()],
            "lint",
            (true, moor_core::workflow::TimeoutMinutes::new(180).unwrap()),
        )
        .unwrap();
        r.cx.store()
            .unwrap()
            .put_workflow(REQUESTER_USER, &def)
            .unwrap()
    };
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let theirs = r.latest_box(other);
    let owner_box = start_owner_box(&r);
    let store = r.cx.store().unwrap();
    // The owner Box's start may already have let a launch watcher mark an agent quiet.
    for id in [mine, other].map(|w| r.latest_box(w)) {
        if store.box_row(id).unwrap().unwrap().session != SessionState::Idle {
            store
                .apply(Decision::new(id).session(SessionInput::AgentQuiet))
                .unwrap();
        }
    }
    r.advance_to(FIRST_DUE + 1_000 + 3 * 60_000);
    r.pass();
    store
        .set_workflow_enabled(
            REQUESTER_USER,
            &WorkflowName::parse("nightly-lint").unwrap(),
            false,
        )
        .unwrap();
    let mine_first = r.latest_box(mine);
    for b in [mine_first, theirs, owner_box] {
        put_to_sleep(&r, b);
    }
    next_day(&r);
    assert_eq!(lifecycle(&r, mine_first), BoxLifecycle::Closed);
    assert_ne!(lifecycle(&r, theirs), BoxLifecycle::Closed);
    assert_ne!(lifecycle(&r, owner_box), BoxLifecycle::Closed);
}

#[test]
fn a_new_start_that_fails_closes_nothing() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    r.forge
        .answer(repo(), Err(moor_core::seam::forge::ForgeError::NotVisible));
    next_day(&r);
    assert!(matches!(
        r.latest(id).outcome,
        moor_core::workflow::WorkflowOutcome::Refused { .. }
    ));
    assert_ne!(lifecycle(&r, first), BoxLifecycle::Closed);
}

/// The `previous_box_kept` reasons recorded on `id`.
fn kept_reasons(r: &Rig, id: BoxId) -> Vec<String> {
    r.cx.store()
        .unwrap()
        .events(id, 1000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "previous_box_kept")
        .map(|e| {
            e.payload
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// Runs the day `n` days after the first, then lets its agent go quiet and its run finish.
fn finished_day(r: &Rig, id: WorkflowId, n: i64) -> BoxId {
    r.advance_to(FIRST_DUE + n * DAY + 1_000);
    r.pass();
    let boxed = r.latest_box(id);
    r.cx.store()
        .unwrap()
        .apply(Decision::new(boxed).session(SessionInput::AgentQuiet))
        .unwrap();
    r.advance_to(FIRST_DUE + n * DAY + 1_000 + 3 * 60_000);
    r.pass();
    boxed
}

#[test]
fn an_asleep_previous_box_with_a_clean_record_closes_without_waking() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    put_to_sleep(&r, first);
    next_day(&r);
    assert_eq!(lifecycle(&r, first), BoxLifecycle::Closed);
    assert!(!event_kinds(&r, first).contains(&"woke".to_owned()));
}

#[test]
fn an_asleep_previous_box_with_no_record_is_kept_without_waking_it() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    put_to_sleep(&r, first);
    r.cx.store()
        .unwrap()
        .set_suspend_check(first, None)
        .unwrap();
    next_day(&r);
    assert_ne!(lifecycle(&r, first), BoxLifecycle::Closed);
    assert!(!event_kinds(&r, first).contains(&"woke".to_owned()));
    assert_eq!(kept_reasons(&r, r.latest_box(id)), ["no stored work check"]);
}

#[test]
fn an_awake_idle_previous_box_is_kept() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    next_day(&r);
    assert_ne!(lifecycle(&r, first), BoxLifecycle::Closed);
    assert_eq!(kept_reasons(&r, r.latest_box(id)), ["awake"]);
}

#[test]
fn a_box_woken_after_it_was_read_asleep_is_kept() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    put_to_sleep(&r, first);
    let store = r.cx.store().unwrap();
    let read = store.box_row(first).unwrap().unwrap();
    assert_eq!(crate::ops::workflow::previous::kept(store, &read), None);
    let name = read.name.clone();
    crate::ops::wake::run(&r.cx, &moor_core::BoxReq { box_name: name }).unwrap();
    store
        .apply(Decision::new(first).session(SessionInput::AgentQuiet))
        .unwrap();
    let why = crate::ops::workflow::previous::keep_or_close(&r.cx, &read);
    assert_eq!(why.as_deref(), Some("awake"));
    assert_ne!(lifecycle(&r, first), BoxLifecycle::Closed);
}

#[test]
fn a_kept_box_closes_once_it_sleeps_clean() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    let second = finished_day(&r, id, 1);
    assert_eq!(kept_reasons(&r, second), ["awake"]);
    put_to_sleep(&r, first);
    finished_day(&r, id, 2);
    assert_eq!(lifecycle(&r, first), BoxLifecycle::Closed);
}

#[test]
fn the_keep_count_counts_only_runs_that_kept_the_same_box() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    finished_first_day(&r, id);
    let second = finished_day(&r, id, 1);
    let third = finished_day(&r, id, 2);
    let fourth = finished_day(&r, id, 3);
    assert_eq!(kept_reasons(&r, second), ["awake"]);
    assert_eq!(
        kept_reasons(&r, third),
        ["awake", "awake; kept by 2 runs in a row"]
    );
    assert_eq!(
        kept_reasons(&r, fourth),
        [
            "awake",
            "awake; kept by 2 runs in a row",
            "needs attention: awake; kept by 3 runs in a row"
        ]
    );
}

#[test]
fn the_same_workflow_on_another_repo_keeps_its_box() {
    let r = rig_probing(CLEAN);
    let other = moor_core::name::ForgeRepo::new("o", "m");
    r.answer_repo();
    r.forge.answer(
        other.clone(),
        Ok(crate::testing::forge::repo_at("abc123", None)),
    );
    let def = moor_core::workflow::WorkflowDef::new(
        WorkflowName::parse("daily-fix").unwrap(),
        moor_core::workflow::WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap(),
        vec![repo(), other.clone()],
        "fix what is broken",
        (true, moor_core::workflow::TimeoutMinutes::new(180).unwrap()),
    )
    .unwrap();
    let store = r.cx.store().unwrap();
    let id = store.put_workflow(REQUESTER_USER, &def).unwrap();
    let box_of = |repo: &moor_core::name::ForgeRepo| {
        r.cx.store()
            .unwrap()
            .recent_workflow_runs(id, repo, 1)
            .unwrap()
            .first()
            .unwrap()
            .outcome
            .box_id()
            .unwrap()
    };
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let (mine, theirs) = (box_of(&repo()), box_of(&other));
    for b in [mine, theirs] {
        store
            .apply(Decision::new(b).session(SessionInput::AgentQuiet))
            .unwrap();
    }
    r.advance_to(FIRST_DUE + 1_000 + 3 * 60_000);
    r.pass();
    put_to_sleep(&r, mine);
    put_to_sleep(&r, theirs);
    r.forge
        .answer(other, Err(moor_core::seam::forge::ForgeError::NotVisible));
    next_day(&r);
    assert_eq!(lifecycle(&r, mine), BoxLifecycle::Closed);
    assert_ne!(lifecycle(&r, theirs), BoxLifecycle::Closed);
}

#[test]
fn every_earlier_box_asleep_and_clean_closes_in_one_run() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    let second = finished_day(&r, id, 1);
    assert_ne!(lifecycle(&r, first), BoxLifecycle::Closed);
    put_to_sleep(&r, first);
    put_to_sleep(&r, second);
    let third = finished_day(&r, id, 2);
    assert_eq!(lifecycle(&r, first), BoxLifecycle::Closed);
    assert_eq!(lifecycle(&r, second), BoxLifecycle::Closed);
    assert_ne!(lifecycle(&r, third), BoxLifecycle::Closed);
}

#[test]
fn the_keep_count_starts_again_after_a_run_that_did_not_keep_the_box() {
    let r = rig_probing(CLEAN);
    r.answer_repo();
    let id = r.put(180);
    finished_first_day(&r, id);
    r.advance_to(FIRST_DUE + DAY + 1_000);
    let job = std::cell::RefCell::new(None);
    crate::ops::workflow::tick(&r.cx, &|j| *job.borrow_mut() = Some(j), &|| false);
    let job = job.into_inner().unwrap();
    let req = moor_core::StartReq {
        name: job.name.clone(),
        prompt: Some(job.prompt.clone()),
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: false,
        cold: false,
        resume: false,
        repo: Some(job.repo),
        also: None,
        parent: None,
        identity: None,
    };
    crate::ops::start::run(&r.cx, &req).unwrap();
    crate::ops::workflow::resolve_interrupted(&r.cx);
    let second = r.latest_box(id);
    assert!(
        kept_reasons(&r, second).is_empty(),
        "a recovered start keeps nothing"
    );
    r.cx.store()
        .unwrap()
        .apply(Decision::new(second).session(SessionInput::AgentQuiet))
        .unwrap();
    r.advance_to(FIRST_DUE + DAY + 1_000 + 3 * 60_000);
    r.pass();
    let third = finished_day(&r, id, 2);
    assert_eq!(kept_reasons(&r, third), ["awake", "awake"]);
}

#[test]
fn an_asleep_previous_box_with_unpushed_work_is_kept_for_that_reason() {
    let r = rig_probing("ahead=0 dirty=3\n");
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    put_to_sleep(&r, first);
    next_day(&r);
    assert_ne!(lifecycle(&r, first), BoxLifecycle::Closed);
    assert_eq!(kept_reasons(&r, r.latest_box(id)), ["unpushed work"]);
}
