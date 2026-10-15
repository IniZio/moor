use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::surface::SurfaceError;
use moor_core::TaskId;

use super::relaunch::{gone, obs, set_agent_session, started};
use super::World;
use crate::ops::start::revive_after_wake;

fn revive(w: &World, id: TaskId) {
    revive_after_wake(&w.cx, w.cx.store().unwrap(), id).unwrap();
}

fn relaunched(w: &World, id: TaskId) -> Vec<serde_json::Value> {
    w.cx.store()
        .unwrap()
        .events(id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "relaunched")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_gone_pane_after_wake_resumes_the_stored_session() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    gone(&w);
    revive(&w, id);
    let all = engine.launches();
    assert_eq!(all.len(), 2);
    assert_eq!(
        all.get(1).and_then(|l| l.resume.as_deref()),
        Some("agent-1")
    );
    let events = relaunched(&w, id);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().and_then(|e| e.get("cause")),
        Some(&"wake".into())
    );
}

#[test]
fn an_exited_pane_after_wake_relaunches_and_keeps_the_old_exit() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.push_pane_obs("p-1", Ok(obs(Some(3))));
    revive(&w, id);
    assert_eq!(engine.launches().len(), 2);
    let exit: Option<i32> =
        w.cx.store()
            .unwrap()
            .reader()
            .unwrap()
            .query_row(
                "SELECT exit FROM sessions WHERE task = ? ORDER BY id LIMIT 1",
                [id.get()],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(exit, Some(3));
    assert_eq!(relaunched(&w, id).len(), 1);
}

#[test]
fn a_live_pane_after_wake_is_left_alone() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    revive(&w, id);
    assert_eq!(engine.launches().len(), 1);
    assert!(relaunched(&w, id).is_empty());
}

#[test]
fn an_unreadable_pane_after_wake_counts_as_live() {
    let (w, engine, id) = started();
    w.surface.push_pane_obs("p-1", Err(SurfaceError::Timeout));
    revive(&w, id);
    assert_eq!(engine.launches().len(), 1);
    assert!(relaunched(&w, id).is_empty());
}

#[test]
fn nothing_to_resume_and_no_goal_launches_nothing() {
    let (w, engine, id) = started();
    let clear = Fields {
        goal: Field::Clear,
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).set(clear))
        .unwrap();
    gone(&w);
    revive(&w, id);
    assert_eq!(engine.launches().len(), 1);
    assert!(relaunched(&w, id).is_empty());
}
