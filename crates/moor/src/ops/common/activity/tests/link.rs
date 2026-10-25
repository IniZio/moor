//! An agent pane that ends while the agent still runs on the Box keeps the session.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::engine::{AgentFate, EngineError};
use moor_core::BoxId;

use super::exit::exited_probes;
use super::{cx, row, run_once, working_box, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::settle;
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

/// Settles one reading of a Box whose pane ended with exit 1, given what the Box answers.
fn settle_exited(fates: Vec<Result<AgentFate, EngineError>>) -> (Cx, Arc<FakeEngine>, BoxId) {
    let dir = tempfile::tempdir().unwrap().keep();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, exited_probes())
            .transcript_text("abc")
            .fates(fates),
    );
    let (cx, _) = cx(&dir, &engine);
    let id = attached_box(&cx);
    let reading = run_once(&cx, id);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &reading).unwrap();
    (cx, engine, id)
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

#[test]
fn a_running_agent_gets_a_new_pane_and_keeps_its_session() {
    let (cx, engine, id) = settle_exited(vec![Ok(AgentFate::Running)]);
    let seen = kinds(&cx, id, WATCHED);
    let names: Vec<&str> = seen.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["agent_link_lost", "agent_link_restored"]);
    assert_eq!(seen[0].1["pane"], "p1");
    assert_eq!(seen[0].1["exit"], 1);
    assert_eq!(seen[1].1["pane"], "reattached-1");
    let live = cx.store().unwrap().live_session(id).unwrap().unwrap();
    assert_eq!(live.pane.as_deref(), Some("reattached-1"));
    assert_eq!(engine.reattached().len(), 1);
    assert!(engine.transcripts().is_empty());
}

#[test]
fn an_ended_agent_ends_the_session_with_the_exit_the_box_kept() {
    let (cx, engine, id) = settle_exited(vec![Ok(AgentFate::Ended { exit: Some(42) })]);
    let seen = kinds(&cx, id, WATCHED);
    let ended = seen.iter().find(|(k, _)| k == "session_ended").unwrap();
    assert_eq!(ended.1["exit"], 42);
    assert!(!seen.iter().any(|(k, _)| k.starts_with("agent_link")));
    assert!(engine.reattached().is_empty());
    assert!(cx.store().unwrap().live_session(id).unwrap().is_none());
}

#[test]
fn a_box_that_stays_unreachable_ends_the_session_and_says_so() {
    let (cx, engine, id) = settle_exited((0..4).map(|_| unreachable()).collect());
    let seen = kinds(&cx, id, WATCHED);
    let lost = seen.iter().find(|(k, _)| k == "agent_unreachable").unwrap();
    assert_eq!(lost.1["attempts"], 4);
    assert!(lost.1["error"]
        .as_str()
        .unwrap()
        .contains("connection refused"));
    let ended = seen.iter().find(|(k, _)| k == "session_ended").unwrap();
    assert_eq!(ended.1["exit"], 1);
    assert!(engine.reattached().is_empty());
}

#[test]
fn a_box_reached_on_a_later_try_reattaches() {
    let (cx, engine, id) =
        settle_exited(vec![unreachable(), unreachable(), Ok(AgentFate::Running)]);
    let seen = kinds(&cx, id, WATCHED);
    assert!(!seen
        .iter()
        .any(|(k, _)| k == "agent_unreachable" || k == "session_ended"));
    assert_eq!(engine.reattached().len(), 1);
}
