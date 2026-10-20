//! `steer` on a Box whose machine cold-boots on wake and loses the agent process.

use std::path::PathBuf;

use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::engine::{SessionRef, Steer};
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::{BoxHandle, BoxPath};

use super::{fx_with, put_to_sleep, run, text, Fx};
use crate::testing::engine::FakeEngine;

fn relaunched(pane: &str, id: &str) -> SessionRef {
    SessionRef {
        id: id.into(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: PathBuf::from("/wt"),
        pane: Some(PaneRef::new(pane)),
        warnings: Vec::new(),
    }
}

/// A sleeping Box with a stored agent session, whose relaunch opens pane `p-2`.
fn asleep() -> Fx {
    let engine = FakeEngine::new().launch_result(Ok(relaunched("p-2", "agent-2")));
    let f = fx_with(engine, true);
    let set = Fields {
        agent_session_id: Field::Set("claude-1".into()),
        workspace_id: Field::Set("ws-1".into()),
        ..Fields::default()
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(set))
        .unwrap();
    put_to_sleep(&f);
    f
}

fn relaunch_events(f: &Fx) -> Vec<serde_json::Value> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter()
        .filter(|e| e.kind == "relaunched")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_cold_boot_wake_relaunches_the_agent_and_the_steer_reaches_it() {
    let f = asleep();
    let surface = f.surface.clone();
    f.driver
        .on_wake(move || surface.push_pane_obs("p-1", Err(SurfaceError::NotFound)));
    run(&f.cx, &text("carry on")).unwrap();
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
    let launches = f.engine.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(
        launches.first().and_then(|l| l.resume.as_deref()),
        Some("claude-1")
    );
    assert_eq!(
        f.engine.steers(),
        vec![("agent-2".to_owned(), Steer::Text("carry on".into()))]
    );
    let events = relaunch_events(&f);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().and_then(|e| e.get("cause")),
        Some(&"wake".into())
    );
}

#[test]
fn a_surviving_agent_is_steered_in_place_after_wake() {
    let f = asleep();
    run(&f.cx, &text("carry on")).unwrap();
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
    assert!(f.engine.launches().is_empty());
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Text("carry on".into()))]
    );
    assert!(relaunch_events(&f).is_empty());
}
