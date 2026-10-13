use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{
    AgentState, HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes, Source,
    STARTUP_GRACE,
};
use moor_core::domain::decision::Decision;
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::{ShowReq, TaskId};

use super::{req, world, World};
use crate::ops::common::activity::read;
use crate::ops::show;
use crate::ops::start::run;
use crate::store::TaskRow;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

const SESSION: &str = "s-1";

/// Probes with `hook` and a pane that reports `pane`.
fn probes(w: &World, hook: Option<HookKind>, pane: PaneStatus) -> Probes {
    let at = Ts::from_millis(w.cx.clock().now().as_millis() - 1000);
    Probes {
        hook: hook.map_or(HookRead::Absent, |kind| {
            HookRead::Fresh(HookRecord {
                kind,
                session_id: None,
                message: None,
                at,
            })
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: pane,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    }
}

/// A started `demo` with a live session, probed as `probes` says; idle first when `idle`.
fn started(idle: bool, probes: impl FnOnce(&World) -> Probes) -> (World, TaskId) {
    let mut w = world(FakeDriver::new());
    let id = TaskId::new(run(&w.cx, &req("demo")).unwrap().task.id);
    let store = w.cx.store().unwrap();
    store.begin_session(id, SESSION, None).unwrap();
    if idle {
        store
            .apply(Decision::new(id).task_input(TaskInput::AgentQuiet))
            .unwrap();
    }
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes(&w)));
    w.cx = w.cx.with_engine(engine);
    (w, id)
}

fn row(w: &World, id: TaskId) -> TaskRow {
    w.cx.store().unwrap().task(id).unwrap().unwrap()
}

/// The `idle_since` column of a task.
fn idle_since(w: &World, id: TaskId) -> Option<i64> {
    w.cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .query_row(
            "SELECT idle_since FROM tasks WHERE id = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn start_idles_a_working_task_whose_hook_reports_idle() {
    let (w, id) = started(false, |w| {
        probes(w, Some(HookKind::Idle), PaneStatus::Unknown)
    });
    assert_eq!(row(&w, id).state, TaskState::Working);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.attached);
    assert_eq!(v.task.state, TaskState::Idle);
    assert_eq!(row(&w, id).state, TaskState::Idle);
    assert!(idle_since(&w, id).is_some());
}

#[test]
fn start_wakes_an_idle_task_whose_hook_reports_working() {
    let (w, id) = started(true, |w| {
        probes(w, Some(HookKind::Busy), PaneStatus::Unknown)
    });
    assert_eq!(row(&w, id).state, TaskState::Idle);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(v.task.state, TaskState::Working);
    assert_eq!(row(&w, id).state, TaskState::Working);
}

/// A started `demo` whose session is older than the startup grace, with no hook report
/// and a fresh pane read of `pane`.
fn aged_pane_only(pane: PaneStatus) -> (World, TaskId) {
    let (w, id) = started(false, |w| probes(w, None, pane));
    w.cx.clock().sleep(STARTUP_GRACE + Duration::from_secs(1));
    (w, id)
}

/// Reads `pane` with no hook report, returning the reading's state, then starts the task.
/// Asserts the pane-only reading never quiets the task.
fn start_on_pane_only(pane: PaneStatus) -> AgentState {
    let (w, id) = aged_pane_only(pane);
    let store = w.cx.store().unwrap();
    let reading = read(&w.cx, store, &row(&w, id), w.cx.clock().now()).unwrap();
    let activity = reading.activity.expect("a live session has a reading");
    assert!(matches!(activity.source, Source::Herdr | Source::Pane));
    assert!(!reading.may_quiet);

    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.attached);
    assert_eq!(v.task.state, TaskState::Working);
    assert_eq!(row(&w, id).state, TaskState::Working);
    activity.state
}

#[test]
fn start_does_not_quiet_a_task_on_a_pane_only_idle_report() {
    assert_eq!(start_on_pane_only(PaneStatus::Idle), AgentState::Idle);
}

#[test]
fn start_does_not_quiet_a_task_on_a_pane_only_blocked_report() {
    assert!(matches!(
        start_on_pane_only(PaneStatus::Blocked),
        AgentState::Blocked { .. }
    ));
}

/// The newest session id of a task.
fn newest_session(w: &World, id: TaskId) -> i64 {
    w.cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .query_row(
            "SELECT MAX(id) FROM sessions WHERE task = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn start_does_not_revive_a_task_whose_session_ended() {
    let (w, id) = started(false, |w| probes(w, None, PaneStatus::Unknown));
    let store = w.cx.store().unwrap();
    while let Some(live) = store.live_session(id).unwrap() {
        store.end_session(live.id, Some(0), None).unwrap();
    }
    let ended = newest_session(&w, id);

    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.attached);

    assert!(store.live_session(id).unwrap().is_none());
    assert_eq!(newest_session(&w, id), ended);

    let list = ShowReq {
        task: None,
        events: None,
        agent: false,
        reports: false,
        pane: false,
    };
    show::run(&w.cx, &list).unwrap();
    assert_eq!(row(&w, id).state, TaskState::Idle);
}

#[test]
fn start_sleeps_an_idle_task_past_the_threshold() {
    let (w, id) = started(true, |w| {
        probes(w, Some(HookKind::Idle), PaneStatus::Unknown)
    });
    w.cx.clock().sleep(Duration::from_mins(31));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(v.task.state, TaskState::Asleep);
    assert_eq!(row(&w, id).state, TaskState::Asleep);
}

/// The kinds of the events of a task.
fn kinds(w: &World, id: TaskId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

#[test]
fn restarting_an_idle_task_whose_agent_is_busy_ends_working_with_a_busy_event() {
    let (w, id) = started(true, |w| {
        probes(w, Some(HookKind::Busy), PaneStatus::Unknown)
    });
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(v.task.state, TaskState::Working);
    assert_eq!(row(&w, id).state, TaskState::Working);
    assert!(kinds(&w, id).contains(&"busy".to_owned()));
}

#[test]
fn restarting_a_working_task_whose_agent_is_busy_leaves_it_working() {
    let (w, id) = started(false, |w| {
        probes(w, Some(HookKind::Busy), PaneStatus::Unknown)
    });
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(v.task.state, TaskState::Working);
    assert_eq!(row(&w, id).state, TaskState::Working);
    assert!(!kinds(&w, id).contains(&"idled".to_owned()));
}

#[test]
fn a_fresh_start_settles_another_task() {
    let (w, id) = started(false, |w| {
        probes(w, Some(HookKind::Idle), PaneStatus::Unknown)
    });
    run(&w.cx, &req("other")).unwrap();
    assert_eq!(row(&w, id).state, TaskState::Idle);
    assert!(kinds(&w, id).contains(&"idled".to_owned()));
}

#[test]
fn a_task_that_cannot_be_settled_warns_and_the_others_still_settle() {
    let mut w = world(FakeDriver::new());
    let bad = TaskId::new(run(&w.cx, &req("bad")).unwrap().task.id);
    let id = TaskId::new(run(&w.cx, &req("demo")).unwrap().task.id);
    w.cx.store()
        .unwrap()
        .begin_session(id, SESSION, None)
        .unwrap();
    let engine = Arc::new(FakeEngine::new().probes(
        SESSION,
        probes(&w, Some(HookKind::Idle), PaneStatus::Unknown),
    ));
    w.cx = w.cx.with_engine(engine);
    let raw = rusqlite::Connection::open(w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO runs (task, argv, sync, pid, started_at) VALUES (?, 'not json', 'none', 1, 1)",
        [bad.get()],
    )
    .unwrap();
    run(&w.cx, &req("other")).unwrap();
    assert_eq!(row(&w, id).state, TaskState::Idle);
    let codes: Vec<_> = w.cx.take_warnings().into_iter().map(|x| x.code).collect();
    assert!(codes.contains(&"settle_failed".to_owned()), "{codes:?}");
}
