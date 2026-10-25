//! Tests for the `turn_done` event written when a steered turn ends.

use std::sync::Arc;

use moor_core::activity::{HookKind, HookRead, Probes};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;

use super::exit::exited_probes;
use super::{cx, hook, row, run_once, settle, working_box, SESSION};
use crate::cx::Cx;
use crate::store::EventRow;
use crate::testing::engine::FakeEngine;

fn fx(probes: Probes) -> (tempfile::TempDir, Cx, BoxId) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    (dir, cx, id)
}

fn steer(cx: &Cx, id: BoxId) -> i64 {
    let event = ExtraEvent::Steered {
        session: SESSION.to_owned(),
        message: "go".to_owned(),
        submit_retried: false,
    };
    let applied = cx
        .store()
        .unwrap()
        .apply(Decision::new(id).event(event))
        .unwrap();
    applied.events.first().copied().unwrap()
}

fn settle_once(cx: &Cx, id: BoxId) {
    let reading = run_once(cx, id);
    settle(cx, cx.store().unwrap(), &row(cx, id), &reading).unwrap();
}

fn events(cx: &Cx, id: BoxId, kind: &str) -> Vec<EventRow> {
    let all = cx.store().unwrap().events(id, 100).unwrap();
    all.into_iter().filter(|e| e.kind == kind).collect()
}

fn first(v: &[EventRow]) -> &EventRow {
    v.first().unwrap()
}

#[test]
fn an_idle_hook_after_a_steer_ends_its_turn_once() {
    let (_dir, cx, id) = fx(hook(HookKind::Idle));
    let steered = steer(&cx, id);
    settle_once(&cx, id);
    settle_once(&cx, id);
    let done = events(&cx, id, "turn_done");
    assert_eq!(done.len(), 1);
    assert_eq!(
        first(&done).payload,
        serde_json::json!({"steer": steered, "outcome": "idle"})
    );
}

#[test]
fn a_blocked_hook_after_a_steer_needs_input() {
    let (_dir, cx, id) = fx(hook(HookKind::Blocked));
    steer(&cx, id);
    settle_once(&cx, id);
    let done = events(&cx, id, "turn_done");
    assert_eq!(done.len(), 1);
    assert_eq!(
        first(&done).payload.get("outcome"),
        Some(&serde_json::json!("needs_input"))
    );
}

#[test]
fn an_exited_pane_after_a_steer_ends_the_turn_before_the_session() {
    let (_dir, cx, id) = fx(exited_probes());
    steer(&cx, id);
    settle_once(&cx, id);
    let done = events(&cx, id, "turn_done");
    assert_eq!(done.len(), 1);
    assert_eq!(
        first(&done).payload.get("outcome"),
        Some(&serde_json::json!("exited"))
    );
    let ended = events(&cx, id, "session_ended");
    assert_eq!(ended.len(), 1);
    assert!(first(&done).id < first(&ended).id);
}

#[test]
fn the_turn_is_noted_before_the_state_step() {
    let (_dir, cx, id) = fx(hook(HookKind::Idle));
    steer(&cx, id);
    settle_once(&cx, id);
    let done = events(&cx, id, "turn_done");
    let idled = events(&cx, id, "idled");
    assert_eq!(idled.len(), 1);
    assert!(first(&done).id < first(&idled).id);
}

#[test]
fn no_steer_writes_no_turn_done() {
    let (_dir, cx, id) = fx(hook(HookKind::Idle));
    settle_once(&cx, id);
    assert!(events(&cx, id, "turn_done").is_empty());
}

#[test]
fn the_end_of_each_turn_copies_the_transcript_to_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, hook(HookKind::Idle))
            .transcript_text("abc"),
    );
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let store = cx.store().unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.set_registered_session(live.id, "agent-1").unwrap();
    settle_once(&cx, id);
    assert!(engine.transcripts().is_empty(), "no turn ended yet");
    steer(&cx, id);
    settle_once(&cx, id);
    let saved = events(&cx, id, "transcript_saved");
    assert_eq!(saved.len(), 1);
    assert_eq!(
        first(&saved)
            .payload
            .get("trigger")
            .and_then(|t| t.as_str()),
        Some("turn")
    );
    assert!(store.live_session(id).unwrap().unwrap().ended_at.is_none());
    steer(&cx, id);
    settle_once(&cx, id);
    assert_eq!(
        engine.transcripts().len(),
        2,
        "the second turn copies again"
    );
    assert_eq!(
        events(&cx, id, "transcript_saved").len(),
        1,
        "an unchanged copy is not recorded"
    );
}

#[test]
fn an_idle_hook_from_before_the_steer_keeps_the_turn_open() {
    let mut probes = hook(HookKind::Idle);
    if let HookRead::Fresh(rec) = &mut probes.hook {
        rec.at = Ts::from_millis(0);
    }
    let (_dir, cx, id) = fx(probes);
    steer(&cx, id);
    settle_once(&cx, id);
    assert!(events(&cx, id, "turn_done").is_empty());
    assert!(events(&cx, id, "idled").is_empty());
    assert!(events(&cx, id, "agent_said").is_empty());
}

#[test]
fn an_idle_hook_after_the_steer_tells_what_the_agent_said() {
    let mut probes = hook(HookKind::Idle);
    if let HookRead::Fresh(rec) = &mut probes.hook {
        rec.message = Some("docs lists one file".to_owned());
    }
    let (_dir, cx, id) = fx(probes);
    steer(&cx, id);
    settle_once(&cx, id);
    let said = events(&cx, id, "agent_said");
    assert_eq!(
        first(&said).payload,
        serde_json::json!({"text": "docs lists one file"})
    );
    let done = events(&cx, id, "turn_done");
    assert!(first(&done).id < first(&said).id);
    assert_eq!(events(&cx, id, "idled").len(), 1);
}
