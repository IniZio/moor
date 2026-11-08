//! An asleep Box is not read, and keeps its session until a wake reads it again.

use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::engine::{AgentFate, EngineError};
use moor_core::BoxId;

use super::exit::exited_probes;
use super::{cx, row, run_once, working_box, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::settle;
use crate::ops::common::agent_stop::stop_agent;
use crate::testing::engine::FakeEngine;

/// A working Box in workspace `ws` whose live session runs in pane `p1`.
fn attached_box(cx: &Cx) -> BoxId {
    let id = working_box(cx, false);
    let store = cx.store().unwrap();
    let set = Fields {
        workspace_id: Field::Set("ws".to_owned()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(set)).unwrap();
    store.begin_session(id, SESSION, Some("p1")).unwrap();
    id
}

/// The kinds of `id`'s events from the given ones on, oldest first.
fn kinds(cx: &Cx, id: BoxId, of: &[&str]) -> Vec<(String, serde_json::Value)> {
    let mut events = cx.store().unwrap().events(id, 100).unwrap();
    events.sort_by_key(|e| e.id);
    events
        .into_iter()
        .filter(|e| of.contains(&e.kind.as_str()))
        .map(|e| (e.kind, e.payload))
        .collect()
}

const WATCHED: &[&str] = &[
    "agent_link_lost",
    "agent_link_restored",
    "agent_unreachable",
    "session_ended",
    "transcript_saved",
    "transcript_skipped",
];

fn unreachable() -> Result<AgentFate, EngineError> {
    Err(EngineError::Failed {
        step: "fate",
        message: "ssh: connection refused".to_owned(),
    })
}

/// A Box whose pane ended with exit 1 once `suspend` put it to sleep, read and settled twice.
fn polled_asleep(suspend: BoxLifecycleInput, quiet: bool) -> (Cx, Arc<FakeEngine>, BoxId) {
    let dir = tempfile::tempdir().unwrap().keep();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, exited_probes())
            .transcript_text("abc")
            .fates(vec![unreachable(); 8]),
    );
    let (cx, _) = cx(&dir, &engine);
    let id = attached_box(&cx);
    let store = cx.store().unwrap();
    if quiet {
        store
            .apply(Decision::new(id).session(SessionInput::AgentQuiet))
            .unwrap();
    }
    store
        .apply(
            Decision::new(id)
                .lifecycle(suspend)
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
    for _ in 0..2 {
        let reading = run_once(&cx, id);
        settle(&cx, store, &row(&cx, id), &reading).unwrap();
    }
    (cx, engine, id)
}

#[test]
fn a_box_suspended_mid_turn_keeps_its_session_while_its_status_is_polled() {
    let suspend = BoxLifecycleInput::Suspend {
        reason: "last-resort suspend".to_owned(),
    };
    let (cx, engine, id) = polled_asleep(suspend, false);
    let store = cx.store().unwrap();
    assert!(store.live_session(id).unwrap().is_some());
    assert_eq!(kinds(&cx, id, WATCHED), []);
    assert_eq!(kinds(&cx, id, &["idled", "turn_done"]), []);
    assert_eq!(engine.fates_left(), 8, "an asleep Box is not asked");
    assert!(!row(&cx, id).session.is_quiet());
}

#[test]
fn a_quiet_box_asleep_keeps_its_session_too() {
    let sleep = BoxLifecycleInput::Sleep {
        auto: true,
        reason: None,
    };
    let (cx, engine, id) = polled_asleep(sleep, true);
    assert!(cx.store().unwrap().live_session(id).unwrap().is_some());
    assert_eq!(kinds(&cx, id, WATCHED), []);
    assert_eq!(engine.fates_left(), 8);
}

#[test]
fn an_asleep_box_is_not_touched_by_settling_and_a_wake_resumes_polling() {
    let suspend = BoxLifecycleInput::Suspend {
        reason: "last-resort suspend".to_owned(),
    };
    let (cx, engine, id) = polled_asleep(suspend, false);
    assert_eq!(
        engine.probe_count(SESSION),
        0,
        "no probe of a stopped guest"
    );
    assert!(engine.asked().is_empty());
    assert!(engine.transcripts().is_empty());
    let store = cx.store().unwrap();
    let wake = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Wake)
        .box_input(BoxInput::Wake);
    store.apply(wake).unwrap();
    engine.set_probes(SESSION, crate::testing::agent::working(super::seen()));
    let reading = run_once(&cx, id);
    settle(&cx, store, &row(&cx, id), &reading).unwrap();
    assert_eq!(engine.probe_count(SESSION), 1, "polling resumes on wake");
    assert!(store.live_session(id).unwrap().is_some());
}

#[test]
fn stopping_the_agent_of_an_asleep_box_does_not_reach_for_the_guest() {
    let suspend = BoxLifecycleInput::Suspend {
        reason: "last-resort suspend".to_owned(),
    };
    let (cx, engine, id) = polled_asleep(suspend, false);
    stop_agent(&cx, cx.store().unwrap(), &row(&cx, id), None).unwrap();
    assert!(engine.asked().is_empty());
}
