//! Tests for the transcript saved when the agent is first seen exited.

use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookRead, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::seam::engine::EngineError;
use moor_core::TaskId;

use super::{cx, row, run_once, task, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::{settle, PaneLiveness, Reading};
use crate::ops::common::boxlock::BoxLock;
use crate::testing::engine::FakeEngine;

fn exited_probes() -> Probes {
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

fn exit_saves(cx: &Cx, id: TaskId) -> usize {
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
    let id = task(&cx, true);
    let reading = run_once(&cx, id);
    assert_eq!(exit_saves(&cx, id), 0);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &reading).unwrap();
    assert_eq!(exit_saves(&cx, id), 1);
    let session = cx.store().unwrap().live_session(id).unwrap().unwrap();
    assert!(session.transcript_digest.is_some());
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    assert_eq!(exit_saves(&cx, id), 1);
    assert_eq!(engine.transcripts().len(), 1);
}

#[test]
fn a_live_or_gone_pane_saves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().transcript_text("abc"));
    let (cx, _) = cx(dir.path(), &engine);
    let id = task(&cx, true);
    for pane in [PaneLiveness::Live, PaneLiveness::Gone] {
        let r = Reading::new(None, pane, false, false);
        settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    }
    assert!(engine.transcripts().is_empty());
}

fn exited_fx(engine: FakeEngine) -> (tempfile::TempDir, Arc<FakeEngine>, Cx, TaskId) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(engine.probes(SESSION, exited_probes()));
    let (cx, _) = cx(dir.path(), &engine);
    let id = task(&cx, true);
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
