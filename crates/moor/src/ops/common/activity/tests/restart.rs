//! A controller restart loses every pane it held while the agents keep running on their Boxes.

use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneRead, Probes};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::engine::{AgentFate, EngineError};
use moor_core::BoxId;

use super::{hook, rec, row, run_once, working_box, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::settle;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    cx: Cx,
    engine: Arc<FakeEngine>,
    surface: Arc<FakeSurface>,
    id: BoxId,
}

/// A working Box whose launch the previous controller boot recorded in pane `stored`, which this
/// boot's surface no longer has.
fn restarted(stored: &str, fates: Vec<Result<AgentFate, EngineError>>) -> Fx {
    let dir = tempfile::tempdir().unwrap().keep();
    let lost = Probes {
        hook: HookRead::Absent,
        pane: PaneRead::Gone,
    };
    let engine = Arc::new(FakeEngine::new().probes(SESSION, lost).fates(fates));
    let surface = Arc::new(FakeSurface::new());
    let cx = Cx::fake(
        &dir,
        Arc::new(FakeDriver::new()),
        surface.clone(),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone());
    let id = working_box(&cx, false);
    let store = cx.store().unwrap();
    let set = Fields {
        workspace_id: Field::Set("pty-ws-1".to_owned()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(set)).unwrap();
    store.begin_session(id, SESSION, Some(stored)).unwrap();
    Fx {
        cx,
        engine,
        surface,
        id,
    }
}

fn settle_once(f: &Fx) {
    let reading = run_once(&f.cx, f.id);
    settle(&f.cx, f.cx.store().unwrap(), &row(&f.cx, f.id), &reading).unwrap();
}

fn count(f: &Fx, kind: &str) -> usize {
    let events = f.cx.store().unwrap().events(f.id, 100).unwrap();
    events.iter().filter(|e| e.kind == kind).count()
}

fn live_pane(f: &Fx) -> Option<String> {
    let live = f.cx.store().unwrap().live_session(f.id).unwrap();
    live.and_then(|s| s.pane)
}

fn unreachable() -> Result<AgentFate, EngineError> {
    Err(EngineError::Failed {
        step: "fate",
        message: "ssh: 502 bad gateway".to_owned(),
    })
}

#[test]
fn a_running_agent_whose_pane_a_restart_lost_is_reattached_without_a_steer() {
    let f = restarted("pty-p-1", vec![Ok(AgentFate::Running)]);
    settle_once(&f);
    assert_eq!(f.engine.reattached().len(), 1);
    assert_eq!(live_pane(&f).as_deref(), Some("reattached-1"));
    assert_eq!(count(&f, "session_ended"), 0);
    assert!(f.engine.steers().is_empty());
}

#[test]
fn the_agents_last_message_is_reported_again_after_the_reattach() {
    let f = restarted("pty-p-1", vec![Ok(AgentFate::Running)]);
    settle_once(&f);
    let mut idle = hook(HookKind::Idle);
    idle.hook = HookRead::Fresh(HookRecord {
        message: Some("Done.".to_owned()),
        ..rec(HookKind::Idle)
    });
    f.engine.set_probes(SESSION, idle);
    settle_once(&f);
    settle_once(&f);
    assert_eq!(count(&f, "agent_said"), 1);
    assert_eq!(count(&f, "idled"), 1);
}

#[test]
fn a_reattach_never_closes_the_pane_it_just_opened() {
    let f = restarted("reattached-1", vec![Ok(AgentFate::Running)]);
    settle_once(&f);
    assert_eq!(f.engine.reattached().len(), 1);
    assert_eq!(live_pane(&f).as_deref(), Some("reattached-1"));
    let calls = f.surface.calls();
    assert!(
        !calls.contains(&"close_pane reattached-1".to_owned()),
        "{calls:?}"
    );
}

#[test]
fn watchers_that_all_saw_the_lost_pane_reattach_it_once() {
    let f = restarted("pty-p-1", (0..3).map(|_| Ok(AgentFate::Running)).collect());
    let readings: Vec<_> = (0..3).map(|_| run_once(&f.cx, f.id)).collect();
    for reading in &readings {
        settle(&f.cx, f.cx.store().unwrap(), &row(&f.cx, f.id), reading).unwrap();
    }
    assert_eq!(f.engine.reattached().len(), 1);
    assert_eq!(count(&f, "agent_link_lost"), 1);
    assert_eq!(count(&f, "agent_link_restored"), 1);
    assert_eq!(live_pane(&f).as_deref(), Some("reattached-1"));
    let calls = f.surface.calls();
    assert!(
        !calls.contains(&"close_pane reattached-1".to_owned()),
        "{calls:?}"
    );
}

#[test]
fn a_link_that_fails_twice_before_it_answers_reattaches_once() {
    let fates = vec![unreachable(), unreachable(), Ok(AgentFate::Running)];
    let f = restarted("pty-p-1", fates);
    settle_once(&f);
    assert_eq!(f.engine.reattached().len(), 1);
    assert_eq!(count(&f, "agent_link_lost"), 1);
    assert_eq!(count(&f, "agent_link_restored"), 1);
    assert_eq!(count(&f, "agent_unreachable"), 0);
}

#[test]
fn a_box_unreachable_for_a_moment_keeps_the_session_and_says_so() {
    let f = restarted("pty-p-1", (0..4).map(|_| unreachable()).collect());
    settle_once(&f);
    assert_eq!(count(&f, "agent_unreachable"), 1);
    assert_eq!(count(&f, "session_ended"), 0);
    assert_eq!(live_pane(&f).as_deref(), Some("pty-p-1"));
}

#[test]
fn a_box_unreachable_past_the_bound_ends_the_session_and_says_why() {
    let f = restarted("pty-p-1", (0..8).map(|_| unreachable()).collect());
    settle_once(&f);
    f.cx.clock().sleep(Duration::from_secs(600));
    settle_once(&f);
    assert_eq!(count(&f, "agent_unreachable"), 2);
    assert_eq!(count(&f, "session_ended"), 1);
    assert!(f.cx.store().unwrap().live_session(f.id).unwrap().is_none());
}

#[test]
fn an_agent_that_ended_while_the_controller_was_down_ends_its_session() {
    let f = restarted("pty-p-1", vec![Ok(AgentFate::Ended { exit: Some(0) })]);
    settle_once(&f);
    assert!(f.engine.reattached().is_empty());
    assert_eq!(count(&f, "session_ended"), 1);
}
