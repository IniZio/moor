//! Tests for the transcript saved when the agent is first seen exited.

use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookRead, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::engine::EngineError;
use moor_core::BoxId;

use super::{cx, row, run_once, task as plain_task, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::{settle, PaneLiveness, Reading};
use crate::ops::common::boxlock::BoxLock;
use crate::testing::engine::FakeEngine;

/// A working task whose live session has a registered agent session.
fn task(cx: &Cx) -> BoxId {
    let id = plain_task(cx, true);
    let store = cx.store().unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.set_registered_session(live.id, "agent-1").unwrap();
    id
}

pub(super) fn exited_probes() -> Probes {
    Probes {
        hook: HookRead::Absent,
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: Some(1),
        }),
    }
}

fn exit_saves(cx: &Cx, id: BoxId) -> usize {
    let events = cx.store().unwrap().events(id, 100).unwrap();
    events
        .iter()
        .filter(|e| {
            e.kind == "transcript_saved"
                && e.payload.get("trigger") == Some(&serde_json::json!("exit"))
        })
        .count()
}

#[test]
fn the_first_exited_settle_saves_the_transcript_once() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, exited_probes())
            .transcript_text("abc"),
    );
    let (cx, _) = cx(dir.path(), &engine);
    let id = task(&cx);
    let reading = run_once(&cx, id);
    assert_eq!(exit_saves(&cx, id), 0);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &reading).unwrap();
    assert_eq!(exit_saves(&cx, id), 1);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    assert_eq!(exit_saves(&cx, id), 1);
    assert_eq!(engine.transcripts().len(), 1);
}

#[test]
fn a_live_or_gone_pane_saves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().transcript_text("abc"));
    let (cx, _) = cx(dir.path(), &engine);
    let id = task(&cx);
    for pane in [PaneLiveness::Live, PaneLiveness::Gone] {
        let r = Reading::new(None, pane, false, false);
        settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    }
    assert!(engine.transcripts().is_empty());
}

fn exited_fx(engine: FakeEngine) -> (tempfile::TempDir, Arc<FakeEngine>, Cx, BoxId) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(engine.probes(SESSION, exited_probes()));
    let (cx, _) = cx(dir.path(), &engine);
    let id = task(&cx);
    (dir, engine, cx, id)
}

#[test]
fn a_failed_exit_save_is_not_retried() {
    let fail = EngineError::Transcript("archive: exit 3".into());
    let (_dir, engine, cx, id) = exited_fx(FakeEngine::new().fail_transcript(fail));
    for _ in 0..3 {
        let r = run_once(&cx, id);
        settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    }
    let failed = cx
        .store()
        .unwrap()
        .events(id, 100)
        .unwrap()
        .iter()
        .filter(|e| e.kind == "transcript_failed")
        .count();
    assert_eq!(failed, 1);
    assert_eq!(engine.transcripts().len(), 1);
}

#[test]
fn a_busy_box_lock_skips_the_exit_save() {
    let (_dir, engine, cx, id) = exited_fx(FakeEngine::new().transcript_text("abc"));
    let held = BoxLock::acquire(
        cx.clock(),
        cx.state_dir().unwrap(),
        id,
        "close",
        Duration::ZERO,
    )
    .unwrap();
    let r = run_once(&cx, id);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    assert!(engine.transcripts().is_empty());
    drop(held);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    assert_eq!(engine.transcripts().len(), 1);
}

#[test]
fn a_session_other_than_the_probed_one_is_not_archived() {
    let (_dir, engine, cx, id) = exited_fx(FakeEngine::new().transcript_text("abc"));
    let mut r = run_once(&cx, id);
    r.session = r.session.map(|s| s + 1);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    assert!(engine.transcripts().is_empty());
}

#[test]
fn a_failed_exit_save_of_one_session_does_not_block_the_next() {
    let fail = EngineError::Transcript("archive: exit 3".into());
    let (_dir, engine, cx, id) = exited_fx(
        FakeEngine::new()
            .fail_transcript(fail)
            .probes("sess-2", exited_probes())
            .transcript_text("abc"),
    );
    let store = cx.store().unwrap();
    let r = run_once(&cx, id);
    settle(&cx, store, &row(&cx, id), &r).unwrap();
    let opened = store.open_session(id).unwrap();
    store.start_session(opened.id).unwrap();
    let second = store.begin_session(id, "sess-2", None).unwrap();
    store.set_registered_session(second, "agent-2").unwrap();
    let r = run_once(&cx, id);
    settle(&cx, store, &row(&cx, id), &r).unwrap();
    assert_eq!(engine.transcripts(), [SESSION, "sess-2"]);
}

#[test]
fn an_exit_save_with_nothing_to_archive_is_recorded_once() {
    let (_dir, engine, cx, id) =
        exited_fx(FakeEngine::new().fail_transcript(EngineError::NothingToArchive));
    for _ in 0..3 {
        let r = run_once(&cx, id);
        settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    }
    let kinds: Vec<_> = cx
        .store()
        .unwrap()
        .events(id, 100)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .filter(|k| k.starts_with("transcript_"))
        .collect();
    assert_eq!(kinds, ["transcript_skipped"]);
    assert_eq!(engine.transcripts().len(), 1);
}

#[test]
fn an_exited_agent_ends_its_session_after_the_save_and_the_task_reads_idle() {
    let (_dir, _engine, cx, id) = exited_fx(FakeEngine::new().transcript_text("abc"));
    let store = cx.store().unwrap();
    for _ in 0..2 {
        let r = run_once(&cx, id);
        settle(&cx, store, &row(&cx, id), &r).unwrap();
    }
    let kinds: Vec<_> = store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "transcript_saved" || e.kind == "session_ended")
        .collect();
    let names: Vec<_> = kinds.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(names, ["transcript_saved", "session_ended"]);
    let ended = &kinds.last().unwrap().payload;
    assert_eq!(ended["cause"], "exited");
    assert_eq!(ended["exit"], 1);
    assert!(store.live_session(id).unwrap().is_none());
    let after = store.task(id).unwrap().unwrap();
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
    assert!(after.session.is_quiet());
}
