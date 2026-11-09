//! Tests for the `turn_done` event written when a steered turn ends.

use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::Probe;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::engine::EngineError;
use moor_core::BoxId;

use super::exit::exited_probes;
use super::{cx, row, run_once, seen, settle, working_box, SESSION};
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::store::EventRow;
use crate::testing::agent::{at_turn, idle, permission, said};
use crate::testing::engine::FakeEngine;

fn fx(probes: Probe) -> (tempfile::TempDir, Cx, BoxId) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    (dir, cx, id)
}

pub(super) fn steer(cx: &Cx, id: BoxId) -> i64 {
    steer_after(cx, id, None)
}

/// A steer sent once the agent's terminal had ended `turn_before` turns.
pub(super) fn steer_after(cx: &Cx, id: BoxId, turn_before: Option<u64>) -> i64 {
    let event = ExtraEvent::Steered {
        session: SESSION.to_owned(),
        message: "go".to_owned(),
        submit_retried: false,
        turn_before,
        status_before: None,
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
fn an_idle_status_after_a_steer_ends_its_turn_once() {
    let (_dir, cx, id) = fx(idle(seen()));
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
fn a_blocked_status_after_a_steer_needs_input() {
    let (_dir, cx, id) = fx(permission("Allow Bash?", seen()));
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
    let (_dir, cx, id) = fx(idle(seen()));
    steer(&cx, id);
    settle_once(&cx, id);
    let done = events(&cx, id, "turn_done");
    let idled = events(&cx, id, "idled");
    assert_eq!(idled.len(), 1);
    assert!(first(&done).id < first(&idled).id);
}

#[test]
fn no_steer_writes_no_turn_done() {
    let (_dir, cx, id) = fx(idle(seen()));
    settle_once(&cx, id);
    assert!(events(&cx, id, "turn_done").is_empty());
}

#[test]
fn the_end_of_each_turn_copies_the_transcript_to_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, idle(seen()))
            .transcript_text("abc"),
    );
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let store = cx.store().unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.set_registered_session(live.id, "agent-1").unwrap();
    settle_once(&cx, id);
    assert_eq!(events(&cx, id, "idled").len(), 1, "the first turn ended");
    assert_eq!(engine.transcripts().len(), 1, "the first turn copies");
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
        3,
        "each later turn copies again"
    );
    assert_eq!(
        events(&cx, id, "transcript_saved").len(),
        1,
        "an unchanged copy is not recorded"
    );
}

#[test]
fn an_idle_agent_whose_ended_turns_have_not_passed_the_steer_keeps_the_turn_open() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, said("Done.", seen())));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    steer_after(&cx, id, Some(1));
    settle_once(&cx, id);
    assert!(events(&cx, id, "turn_done").is_empty());
    assert!(events(&cx, id, "idled").is_empty());
    assert!(events(&cx, id, "agent_said").is_empty());
    engine.set_probes(SESSION, at_turn(said("Done.", seen()), 2));
    settle_once(&cx, id);
    assert_eq!(events(&cx, id, "turn_done").len(), 1);
    assert_eq!(events(&cx, id, "agent_said").len(), 1);
    assert_eq!(events(&cx, id, "idled").len(), 1);
}

#[test]
fn an_idle_status_after_the_steer_tells_what_the_agent_said() {
    let (_dir, cx, id) = fx(said("docs lists one file", seen()));
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

/// A Box gone Idle with its first turn's transcript saved, and a registered agent session.
fn saved_once(engine: &Arc<FakeEngine>) -> (tempfile::TempDir, Cx, BoxId) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path(), engine);
    let id = working_box(&cx, true);
    let store = cx.store().unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.set_registered_session(live.id, "agent-1").unwrap();
    settle_once(&cx, id);
    assert_eq!(engine.transcripts().len(), 1, "the first turn saves");
    (dir, cx, id)
}

#[test]
fn a_turn_end_seen_while_the_box_is_busy_is_marked_and_saved_on_the_next_pass() {
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, idle(seen()))
            .transcript_text("abc"),
    );
    let (_dir, cx, id) = saved_once(&engine);
    steer(&cx, id);
    let held = cx.owners_arc().hold(id, Cmd::Close { force: false });
    settle_once(&cx, id);
    assert!(events(&cx, id, "turn_done").is_empty(), "busy: no turn end");
    assert_eq!(cx.store().unwrap().archive_due(id).unwrap(), None);
    assert_eq!(engine.transcripts().len(), 1, "busy: nothing saved");
    drop(held);
    settle_once(&cx, id);
    assert_eq!(events(&cx, id, "turn_done").len(), 1);
    assert_eq!(engine.transcripts().len(), 2, "the next pass saves");
    settle_once(&cx, id);
    assert_eq!(engine.transcripts().len(), 2, "a paid due saves no more");
}

#[test]
fn a_failed_turn_save_is_recorded_and_tried_again_a_minute_later_until_it_lands() {
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, idle(seen()))
            .fail_transcript(EngineError::Transcript("archive: exit 3".into()))
            .transcript_text("abc"),
    );
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let store = cx.store().unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.set_registered_session(live.id, "agent-1").unwrap();
    settle_once(&cx, id);
    assert_eq!(events(&cx, id, "transcript_failed").len(), 1);
    settle_once(&cx, id);
    assert_eq!(engine.transcripts().len(), 1, "not again at once");
    cx.clock().sleep(Duration::from_mins(1));
    settle_once(&cx, id);
    assert_eq!(engine.transcripts().len(), 2, "again a minute later");
    assert_eq!(events(&cx, id, "transcript_saved").len(), 1);
}
