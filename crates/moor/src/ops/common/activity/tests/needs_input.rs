//! Tests for the `needs_input` event written once per waiting episode.

use std::sync::Arc;

use moor_core::activity::{Activity, AgentState, Source};
use moor_core::BoxId;

use super::{cx, row, working_box};
use crate::cx::Cx;
use crate::ops::common::activity::{settle, PaneLiveness, Reading};
use crate::testing::engine::FakeEngine;

fn reading(state: AgentState) -> Reading {
    let activity = Activity {
        state,
        source: Source::Pane,
    };
    Reading::new(Some(activity), PaneLiveness::Live, true, false)
}

fn asking(q: &str) -> Reading {
    reading(AgentState::NeedsInput {
        question: Some(q.to_owned()),
        options: vec!["1. Yes".to_owned(), "2. No".to_owned()],
    })
}

fn pass(cx: &Cx, id: BoxId, r: &Reading) {
    settle(cx, cx.store().unwrap(), &row(cx, id), r).unwrap();
}

fn kinds(cx: &Cx, id: BoxId) -> Vec<String> {
    let events = cx.store().unwrap().events(id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn asked(cx: &Cx, id: BoxId) -> usize {
    kinds(cx, id).iter().filter(|k| *k == "needs_input").count()
}

fn setup() -> (tempfile::TempDir, Cx, BoxId) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path(), &Arc::new(FakeEngine::new()));
    let id = working_box(&cx, true);
    (dir, cx, id)
}

#[test]
fn one_event_per_waiting_episode_across_passes() {
    let (_dir, cx, id) = setup();
    pass(&cx, id, &asking("Proceed?"));
    pass(&cx, id, &asking("Proceed?"));
    assert_eq!(asked(&cx, id), 1);
}

#[test]
fn the_event_survives_the_idle_step_and_carries_the_prompt() {
    let (_dir, cx, id) = setup();
    pass(&cx, id, &asking("Proceed?"));
    let events = cx.store().unwrap().events(id, 100).unwrap();
    let ev = events.iter().find(|e| e.kind == "needs_input").unwrap();
    assert_eq!(
        ev.payload,
        serde_json::json!({"question": "Proceed?", "options": ["1. Yes", "2. No"]})
    );
    assert!(kinds(&cx, id).contains(&"idled".to_owned()));
}

#[test]
fn answering_ends_the_episode() {
    let (_dir, cx, id) = setup();
    pass(&cx, id, &asking("Proceed?"));
    pass(&cx, id, &reading(AgentState::Working));
    assert!(kinds(&cx, id).contains(&"busy".to_owned()));
    pass(&cx, id, &asking("Proceed?"));
    assert_eq!(asked(&cx, id), 2);
}

#[test]
fn a_different_question_without_a_resume_is_a_new_event() {
    let (_dir, cx, id) = setup();
    pass(&cx, id, &asking("Proceed?"));
    pass(&cx, id, &asking("Delete it?"));
    assert_eq!(asked(&cx, id), 2);
}
